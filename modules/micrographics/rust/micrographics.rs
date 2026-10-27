//! Micrographics — ambient weather feed for the desktop panel.
//!
//! Rust half of the `micrographics` vertical slice. The interface composes the
//! observation into the glyph panel; this crate only owns *fetching* and
//! *caching* it. Talks to the kernel strictly through `unidtk-api`: no
//! dependency on the kernel crate, none on other modules.
//!
//! ## Why nothing here blocks a caller
//!
//! The panel is ambient: it redraws on a one-second tick, and asking the kernel
//! "what is the weather" must never be able to stall that tick — or, worse, the
//! Tauri UI thread. So the rule here is absolute:
//!
//! - the **only** place a network request happens is the background refresh
//!   thread started from [`Module::init`];
//! - the door handlers are pure reads of an in-memory snapshot, answered in
//!   microseconds whether the network is up or not.
//!
//! The cost of that choice is honest and small: on a cold start the panel shows
//! nothing weather-shaped for a few seconds until the first fetch lands. The
//! alternative — fetching inline so the very first paint is already warm — would
//! put a multi-second network round-trip on the door the UI thread waits on.
//!
//! ## Location
//!
//! Windows positioning is the sole source of weather coordinates. Access
//! is requested by an explicit foreground UI action; this worker never prompts.
//! Public-IP information is an independent network indicator, never a weather
//! fallback. Windows may itself estimate a position from its network provider;
//! its method and accuracy are published explicitly, never called a device fix.
//! Old standalone IP-derived weather caches are deliberately not restored.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Refresh cadence once things are working. A current-conditions observation does
/// not move fast enough to be worth more CPU than this, and the panel is ambient.
const REFRESH: Duration = Duration::from_secs(15 * 60);

/// Longest wait between attempts once the provider is failing. A fixed short
/// cadence against an unreachable or rate-limiting provider is how a widget gets
/// the app's network reputation — so failures push the next attempt out.
const BACKOFF_MAX: Duration = Duration::from_secs(5 * 60);

/// How long a snapshot may be served before the payload says it is stale.
/// Comfortably above [`REFRESH`], so a normal cycle never shows the warning.
const FRESH_FOR: u64 = 20 * 60;

/// Public-IP identity refreshes independently every thirty minutes. Weather
/// coordinates are obtained anew from the Windows device API each cycle.
const LOCATION_FOR: u64 = 30 * 60;
const GEO_TIMEOUT: Duration = Duration::from_secs(20);
const NAME_FOR: u64 = 30 * 60;

/// The narrowest radius within which a place name may still be trusted.
///
/// Windows publishes the uncertainty of its own fix, and a network estimate is
/// honest only inside that radius: the claim about a place can never be
/// tighter than the fix it is based on. A Wi-Fi fix claims metres, an IP
/// estimate claims kilometres, and a name must follow the weaker of the two.
const PLACE_MIN_RADIUS_M: f64 = 1_000.0;

/// How long a provider that could not be reached is left alone. A blocked host
/// must not be probed on every refresh cycle, but a network hiccup must not
/// cost the provider its place in the chain for long.
const PLACE_UNREACHABLE_COOLDOWN: u64 = 10 * 60;

/// How long a provider that refused the request is left alone. A refusal is a
/// statement about the client, not about the network, and repeating it is how
/// a provider ends up banning the whole application.
const PLACE_REFUSED_COOLDOWN: u64 = 60 * 60;

/// Network budget per request. Short on purpose — a widget that answers in five
/// seconds has already outlived its usefulness, and a short budget is also what
/// keeps shutdown inside the kernel's per-module allowance.
const HTTP_TIMEOUT: Duration = Duration::from_secs(4);

const UA: &str = "unidtk-micrographics/1.0";

/// Key layout inside the module's own cache store.
const KEY_SNAPSHOT: &str = "windows/snapshot-v2";
const KEY_IP: &str = "network/ip-location-v1";
const KEY_ACCESS: &str = "windows/access-allowed-v1";

/// Separate clocks keep an unavailable provider from delaying another feed.
struct RefreshSchedule {
    due: Instant,
    failures: u32,
    normal: Duration,
}
impl RefreshSchedule {
    fn new(normal: Duration) -> Self { Self { due: Instant::now(), failures: 0, normal } }
    fn finish(&mut self, success: bool) {
        self.failures = if success { 0 } else { self.failures.saturating_add(1) };
        let delay = if success { self.normal } else { retry_delay(self.failures) };
        self.due = Instant::now() + delay;
    }
    fn wake(&mut self) { self.due = Instant::now(); self.failures = 0; }
}
fn retry_delay(failures: u32) -> Duration {
    Duration::from_secs(5).saturating_mul(1u32 << failures.saturating_sub(1).min(6)).min(BACKOFF_MAX)
}
/// ureq 2 applies neither its request nor connect deadline to system DNS.
/// One bounded resolver service avoids accumulating detached requests on a
/// broken network while keeping the feed worker's wait inside its budget.
fn bounded_resolve(address: &str) -> std::io::Result<Vec<std::net::SocketAddr>> {
    use std::{
        collections::HashMap,
        net::{SocketAddr, ToSocketAddrs},
        sync::{mpsc, Mutex, OnceLock},
        time::{Duration, Instant},
    };
    static DNS_CACHE: OnceLock<Mutex<HashMap<String, (Vec<SocketAddr>, Instant)>>> = OnceLock::new();
    let cache = DNS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some((addrs, expires_at)) = guard.get(address) {
            if Instant::now() < *expires_at && !addrs.is_empty() {
                return Ok(addrs.clone());
            }
        }
    }

    let (send, receive) = mpsc::sync_channel::<std::io::Result<Vec<SocketAddr>>>(1);
    let target = address.to_owned();
    let _ = thread::Builder::new().name("weather-dns".into()).spawn(move || {
        let result = target.to_socket_addrs().map(|addresses| addresses.collect());
        let _ = send.send(result);
    });

    let addrs: Vec<SocketAddr> = receive.recv_timeout(HTTP_TIMEOUT)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "weather DNS resolution timed out"))??;

    if !addrs.is_empty() {
        if let Ok(mut guard) = cache.lock() {
            guard.insert(address.to_owned(), (addrs.clone(), Instant::now() + Duration::from_secs(15 * 60)));
        }
    }
    Ok(addrs)
}

fn http_agent() -> ureq::Agent {
    ureq::AgentBuilder::new().resolver(bounded_resolve).timeout_connect(HTTP_TIMEOUT)
        .timeout_read(HTTP_TIMEOUT).timeout_write(HTTP_TIMEOUT).timeout(HTTP_TIMEOUT).build()
}

/// One current-conditions observation, as the panel consumes it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeatherSnapshot {
    /// Air temperature, °C, rounded to a whole degree. The panel has no room for
    /// fractions, and a tenth of a degree is noise rather than information.
    pub temp_c: Option<i32>,
    /// Apparent temperature, °C, when reported by the provider.
    pub feels_c: Option<i32>,
    /// Wind speed, km/h.
    pub wind_kmh: Option<u32>,
    /// WMO weather code as received, kept verbatim so the interface can label it
    /// without this crate owning a translation table the kernel never uses.
    pub code: Option<u8>,
    /// Whether it is day at the observed location (drives the micro-glyph).
    pub is_day: Option<bool>,
    /// Where the observation was made, as the provider named it.
    pub location: String,
    #[serde(default = "unavailable_name_status")]
    pub location_name_status: String,
    /// Who named the place, for attribution in the panel: `bigdatacloud`,
    /// `openstreetmap`, or `windows` for a place Windows itself provides.
    #[serde(default)]
    pub location_name_provider: Option<String>,
    /// When the observation was taken, unix seconds.
    pub observed_at: u64,
    /// True when the observation is older than [`FRESH_FOR`]: the network is
    /// unreachable, not the weather interesting. The panel renders it as a muted,
    /// struck-through value rather than silently presenting old numbers as current.
    pub stale: bool,
    pub weather_available: bool,
    pub location_source: Option<String>,
    #[serde(default)]
    pub location_method: Option<String>,
    #[serde(default)]
    pub accuracy_m: Option<f64>,
    pub location_status: String,
    pub ip_location: Option<IpLocation>,
    #[serde(default)]
    pub precipitation_forecast: Vec<PrecipitationHour>,
    #[serde(default)]
    pub sunrise_at: Option<u64>,
    #[serde(default)]
    pub sunset_at: Option<u64>,
    #[serde(default)]
    pub daylight_seconds: Option<u32>,
    #[serde(default)]
    pub timezone: Option<String>,
}

/// Provider hourly precipitation refers to the hour ENDING at `at`, in UTC.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrecipitationHour {
    pub at: u64,
    pub probability: Option<u8>,
    pub amount_mm: Option<f64>,
}

impl WeatherSnapshot {
    fn empty(status: &str) -> Self {
        Self {
            temp_c: None,
            feels_c: None,
            wind_kmh: None,
            code: None,
            is_day: None,
            location: String::new(),
            location_name_status: unavailable_name_status(),
            location_name_provider: None,
            observed_at: 0,
            stale: false,
            weather_available: false,
            location_source: None,
            location_method: None,
            accuracy_m: None,
            location_status: status.into(),
            ip_location: None,
            precipitation_forecast: Vec::new(),
            sunrise_at: None,
            sunset_at: None,
            daylight_seconds: None,
            timezone: None,
        }
    }
    fn clear_weather(&mut self, status: &str) {
        let ip = self.ip_location.take();
        *self = Self::empty(status);
        self.ip_location = ip;
    }
}

fn unavailable_name_status() -> String {
    "unavailable".into()
}

/// Public network identity. It never carries weather coordinates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpLocation {
    pub city: String,
    pub country: String,
    pub ip: String,
    pub resolved_at: u64,
    pub stale: bool,
}

/// A device fix used only by the background weather request. Coordinates never
/// enter the public snapshot or the socket read door.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Location {
    latitude: f64,
    longitude: f64,
    name: String,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    accuracy_m: Option<f64>,
    /// When the lookup ran, unix seconds.
    resolved_at: u64,
}

impl Location {
    /// Reject malformed or placeholder device coordinates before a request.
    fn is_plausible(&self) -> bool {
        self.latitude.is_finite()
            && self.longitude.is_finite()
            && self.latitude.abs() <= 90.0
            && self.longitude.abs() <= 180.0
            && (self.latitude != 0.0 || self.longitude != 0.0)
    }
}

/// Coordinates stay in process memory: a fresh Windows fix must match before
/// a previous city name can be reused, including after permission changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct PlaceName {
    latitude: f64,
    longitude: f64,
    name: String,
    method: Option<String>,
    /// Who named the place, for attribution in the panel.
    #[serde(default)]
    provider: Option<String>,
    resolved_at: u64,
}

impl PlaceName {
    fn matches(&self, fix: &Location, now: u64) -> bool {
        if self.method != fix.method || now.saturating_sub(self.resolved_at) > NAME_FOR {
            return false;
        }
        self.describes_same_place(fix)
    }

    /// The same place, ignoring freshness: the name may be outdated, but the
    /// place it names is still the one the device is in. A failed refresh must
    /// not erase a name that is still true.
    fn describes_same_place(&self, fix: &Location) -> bool {
        self.method == fix.method
            && distance_m(self.latitude, self.longitude, fix.latitude, fix.longitude)
                <= place_match_radius_m(fix)
    }
}

/// How far a fix may wander before the place it names counts as another one.
fn place_match_radius_m(fix: &Location) -> f64 {
    let claimed = fix.accuracy_m.unwrap_or(0.0);
    if claimed.is_finite() && claimed > PLACE_MIN_RADIUS_M {
        claimed
    } else {
        PLACE_MIN_RADIUS_M
    }
}

/// Whether two fixes describe one place. The uncertainty of both sides counts:
/// a network estimate is only as precise as it claims to be.
fn fixes_describe_same_place(previous: &Location, current: &Location) -> bool {
    previous.method == current.method
        && distance_m(previous.latitude, previous.longitude, current.latitude, current.longitude)
            <= place_match_radius_m(previous).max(place_match_radius_m(current))
}

fn distance_m(latitude: f64, longitude: f64, other_latitude: f64, other_longitude: f64) -> f64 {
    let lat1 = latitude.to_radians();
    let lat2 = other_latitude.to_radians();
    let a = ((lat2 - lat1) / 2.0).sin().powi(2)
        + lat1.cos() * lat2.cos() * ((other_longitude - longitude).to_radians() / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * a.clamp(0.0, 1.0).sqrt().asin()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlaceResponse {
    latitude: Option<f64>,
    longitude: Option<f64>,
    lookup_source: Option<String>,
    city: Option<String>,
    locality: Option<String>,
    principal_subdivision: Option<String>,
}

fn place_name_from_response(parsed: PlaceResponse, fix: &Location) -> Result<String, String> {
    if !matches!(parsed.lookup_source.as_deref(), Some("coordinates" | "reverseGeocoding")) {
        return Err("place lookup did not use device coordinates".into());
    }
    let same_coordinate = |actual: Option<f64>, expected: f64| {
        actual.is_some_and(|value| value.is_finite() && (value - expected).abs() <= 0.00001)
    };
    if !fix.is_plausible()
        || !same_coordinate(parsed.latitude, fix.latitude)
        || !same_coordinate(parsed.longitude, fix.longitude)
    {
        return Err("place lookup returned a different location".into());
    }
    [parsed.city, parsed.locality, parsed.principal_subdivision]
        .into_iter()
        .flatten()
        .map(|name| name.trim().to_string())
        .find(|name| !name.is_empty() && name.chars().count() <= 160 && !name.chars().any(char::is_control))
        .ok_or_else(|| "place lookup returned no city or locality".into())
}

/// Why a place lookup failed. The kind decides how long the provider is left
/// alone: a refused request needs a long pause, an unreachable host a short
/// one, and an unusable answer none at all.
enum PlaceLookupError {
    /// The provider could not be reached: no route, no handshake, timeout.
    Unreachable(String),
    /// The provider answered with a refusal (403/429).
    Refused(String),
    /// The provider answered, but the answer cannot be trusted.
    Unusable(String),
}

impl PlaceLookupError {
    fn cooldown(&self) -> u64 {
        match self {
            Self::Unreachable(_) => PLACE_UNREACHABLE_COOLDOWN,
            Self::Refused(_) => PLACE_REFUSED_COOLDOWN,
            Self::Unusable(_) => 0,
        }
    }
}

impl std::fmt::Display for PlaceLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(message) | Self::Refused(message) | Self::Unusable(message) => {
                f.write_str(message)
            }
        }
    }
}

/// Who can name the place the device claims to be in.
///
/// Two independent providers, because a network filter may block one host
/// while the other stays reachable — and a blocked host must never be the
/// reason the panel says the place is unknown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PlaceProvider {
    BigDataCloud,
    OpenStreetMap,
    /// The place Windows itself provides; no geocoder is asked.
    Windows,
}

static BIGDATACLOUD_COOLDOWN: AtomicU64 = AtomicU64::new(0);
static OPENSTREETMAP_COOLDOWN: AtomicU64 = AtomicU64::new(0);

impl PlaceProvider {
    fn id(self) -> &'static str {
        match self {
            Self::BigDataCloud => "bigdatacloud",
            Self::OpenStreetMap => "openstreetmap",
            Self::Windows => "windows",
        }
    }
    fn cooldown(self) -> Option<&'static AtomicU64> {
        match self {
            Self::BigDataCloud => Some(&BIGDATACLOUD_COOLDOWN),
            Self::OpenStreetMap => Some(&OPENSTREETMAP_COOLDOWN),
            Self::Windows => None,
        }
    }
    fn lookup(self, fix: &Location) -> Result<String, PlaceLookupError> {
        match self {
            Self::BigDataCloud => lookup_place_name_bigdatacloud(fix),
            Self::OpenStreetMap => lookup_place_name_openstreetmap(fix),
            Self::Windows => Ok("Место задано в Windows".into()),
        }
    }
}

/// The name of the place the device claims to be in, and who named it.
fn lookup_place_name(fix: &Location) -> Result<(String, PlaceProvider), String> {
    lookup_place_name_with(
        &[PlaceProvider::BigDataCloud, PlaceProvider::OpenStreetMap],
        fix,
        |provider, fix| provider.lookup(fix),
    )
}

/// The chain itself, with the provider call injected, so the order, the
/// fallover and the cooldowns are testable without a network.
fn lookup_place_name_with(
    providers: &[PlaceProvider],
    fix: &Location,
    mut lookup: impl FnMut(PlaceProvider, &Location) -> Result<String, PlaceLookupError>,
) -> Result<(String, PlaceProvider), String> {
    if fix.method.as_deref() == Some("default") {
        return Ok(("Место задано в Windows".into(), PlaceProvider::Windows));
    }
    if !fix.is_plausible() {
        return Err("place lookup requires a valid device fix".into());
    }
    let mut problems: Vec<String> = Vec::new();
    for provider in providers {
        let Some(cooldown) = provider.cooldown() else {
            continue;
        };
        if cooldown.load(Ordering::Acquire) > now_unix() {
            problems.push(format!("{}: cooling down", provider.id()));
            continue;
        }
        match lookup(*provider, fix) {
            Ok(name) => return Ok((name, *provider)),
            Err(error) => {
                let pause = error.cooldown();
                if pause > 0 {
                    cooldown.store(now_unix().saturating_add(pause), Ordering::Release);
                }
                problems.push(format!("{}: {error}", provider.id()));
            }
        }
    }
    if problems.is_empty() {
        Err("place lookup has no provider available".into())
    } else {
        Err(problems.join("; "))
    }
}

fn lookup_place_name_bigdatacloud(fix: &Location) -> Result<String, PlaceLookupError> {
    // Native client, current device fix only. This API explicitly supports
    // direct client reverse geocoding; never invoke its no-coordinate IP path.
    let response = http_agent().get("https://api.bigdatacloud.net/data/reverse-geocode-client")
        .query("latitude", &fix.latitude.to_string())
        .query("longitude", &fix.longitude.to_string())
        .query("localityLanguage", "ru")
        .set("User-Agent", UA)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|error| match &error {
            // Only the status is kept: the body and the request URL are not.
            ureq::Error::Status(status, _) if *status == 403 || *status == 429 => {
                PlaceLookupError::Refused(format!("place lookup refused with status {status}"))
            }
            ureq::Error::Status(status, _) => {
                PlaceLookupError::Unreachable(format!("place lookup failed with status {status}"))
            }
            _ => PlaceLookupError::Unreachable("place lookup request failed".into()),
        })?;
    let body = response
        .into_string()
        .map_err(|_| PlaceLookupError::Unreachable("place lookup read failed".into()))?;
    let parsed: PlaceResponse = serde_json::from_str(&body)
        .map_err(|_| PlaceLookupError::Unusable("place lookup response invalid".into()))?;
    place_name_from_response(parsed, fix).map_err(PlaceLookupError::Unusable)
}

fn lookup_place_name_openstreetmap(fix: &Location) -> Result<String, PlaceLookupError> {
    let response = http_agent()
        .get("https://nominatim.openstreetmap.org/reverse")
        .query("format", "jsonv2")
        // A settlement rather than a street corner: the name of a place, and a
        // bounding box that holds the point that was asked about.
        .query("zoom", "12")
        .query("lat", &fix.latitude.to_string())
        .query("lon", &fix.longitude.to_string())
        .query("accept-language", "ru")
        .set("User-Agent", UA)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|error| match &error {
            ureq::Error::Status(status, _) if *status == 403 || *status == 429 => {
                PlaceLookupError::Refused(format!("place lookup refused with status {status}"))
            }
            ureq::Error::Status(status, _) => {
                PlaceLookupError::Unreachable(format!("place lookup failed with status {status}"))
            }
            _ => PlaceLookupError::Unreachable("place lookup request failed".into()),
        })?;
    let body = response
        .into_string()
        .map_err(|_| PlaceLookupError::Unreachable("place lookup read failed".into()))?;
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .map_err(|_| PlaceLookupError::Unusable("place lookup response invalid".into()))?;
    nominatim_place_name(&parsed, fix).map_err(PlaceLookupError::Unusable)
}

/// The name OpenStreetMap reports for a point, and the check that the answer
/// is about the place the device claims to be in.
///
/// The service answers with the *centroid* of the matched feature and its
/// bounding box, never with an echo of the requested point: the coordinates in
/// the answer may sit far from the query. The bounding box is what ties the
/// name to the place, so the queried point must lie inside it.
fn nominatim_place_name(parsed: &serde_json::Value, fix: &Location) -> Result<String, String> {
    if parsed.get("error").is_some() {
        return Err("place lookup returned an error".into());
    }
    let Some(bounds) = parsed.get("boundingbox").and_then(|value| value.as_array()) else {
        return Err("place lookup returned no bounding box".into());
    };
    if bounds.len() != 4 {
        return Err("place lookup returned a malformed bounding box".into());
    }
    let mut numbers = [0.0f64; 4];
    for (slot, value) in numbers.iter_mut().zip(bounds.iter()) {
        *slot = json_number(value).ok_or("place lookup returned a malformed bounding box")?;
    }
    let [min_lat, max_lat, min_lon, max_lon] = numbers;
    if min_lat > max_lat || min_lon > max_lon {
        return Err("place lookup returned an inverted bounding box".into());
    }
    if fix.latitude < min_lat
        || fix.latitude > max_lat
        || fix.longitude < min_lon
        || fix.longitude > max_lon
    {
        return Err("place lookup returned a different location".into());
    }
    let Some(address) = parsed.get("address").and_then(|value| value.as_object()) else {
        return Err("place lookup returned no address".into());
    };
    // The nearest settlement first, the region last: in Russia `state` is the
    // federal subject and `region` the federal district — neither is a place.
    ["city", "town", "village", "municipality", "city_district", "county", "state"]
        .into_iter()
        .filter_map(|field| address.get(field).and_then(|value| value.as_str()))
        .map(|name| name.trim().to_string())
        .find(|name| !name.is_empty() && name.chars().count() <= 160 && !name.chars().any(char::is_control))
        .ok_or_else(|| "place lookup returned no city or locality".into())
}

/// A number the service may have written as a string.
fn json_number(value: &serde_json::Value) -> Option<f64> {
    let raw = match value {
        serde_json::Value::Number(number) => number.as_f64()?,
        serde_json::Value::String(text) => text.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    raw.is_finite().then_some(raw)
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ─── Open-Meteo ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MeteoCurrent {
    #[serde(default, deserialize_with = "optional_number")]
    temperature_2m: Option<f64>,
    #[serde(
        default,
        deserialize_with = "optional_number",
        rename = "apparent_temperature"
    )]
    apparent_temperature: Option<f64>,
    #[serde(
        default,
        deserialize_with = "optional_number",
        rename = "wind_speed_10m"
    )]
    wind_speed_10m: Option<f64>,
    #[serde(default, deserialize_with = "optional_number", rename = "is_day")]
    is_day: Option<f64>,
    #[serde(default, deserialize_with = "optional_number", rename = "weather_code")]
    weather_code: Option<f64>,
}

fn optional_number<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<f64>, D::Error> {
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(value.as_f64().filter(|number| number.is_finite()))
}

#[derive(Deserialize)]
struct MeteoResponse {
    current: Option<MeteoCurrent>,
    #[serde(default)]
    hourly: serde_json::Value,
    #[serde(default)]
    daily: serde_json::Value,
    #[serde(default)]
    timezone: serde_json::Value,
    #[serde(default)]
    utc_offset_seconds: serde_json::Value,
}

fn array_number(value: &serde_json::Value, key: &str, index: usize) -> Option<f64> {
    value.get(key)?.as_array()?.get(index)?.as_f64().filter(|v| v.is_finite())
}

fn array_epoch(value: &serde_json::Value, key: &str, index: usize) -> Option<u64> {
    value.get(key)?.as_array()?.get(index)?.as_u64().filter(|v| *v > 0 && *v <= 253_402_300_799)
}

fn precipitation_hours(hourly: &serde_json::Value, now: u64) -> Vec<PrecipitationHour> {
    let Some(times) = hourly.get("time").and_then(|v| v.as_array()) else { return Vec::new(); };
    // Reject gaps rather than describing an arbitrary collection as three hours.
    let Some(start) = times.iter().position(|v| v.as_u64().is_some_and(|t| t <= 253_402_300_799 && t > now && t - now <= 3600)) else { return Vec::new(); };
    let Some(first) = times[start].as_u64() else { return Vec::new(); };
    // One reserve endpoint keeps three live windows across an hour boundary
    // between normal refreshes. The reserve is optional, never a new request.
    let mut hours = Vec::with_capacity(4);
    for step in 0..4 {
        let index = start + step;
        let at = times.get(index).and_then(|v| v.as_u64())
            .filter(|at| *at <= 253_402_300_799 && *at == first + step as u64 * 3600);
        let Some(at) = at else {
            if step == 3 { break; }
            return Vec::new();
        };
        hours.push(PrecipitationHour {
            at,
            probability: array_number(hourly, "precipitation_probability", index)
                .filter(|v| *v >= 0.0 && *v <= 100.0 && v.fract() == 0.0).map(|v| v as u8),
            amount_mm: array_number(hourly, "precipitation", index)
                .filter(|v| *v >= 0.0 && *v <= 1000.0),
        });
    }
    hours
}

fn daylight_values(daily: &serde_json::Value, offset: &serde_json::Value, now: u64) -> (Option<u64>, Option<u64>, Option<u32>) {
    let Some(offset) = offset.as_i64().filter(|v| (-86400..=86400).contains(v)) else { return (None, None, None); };
    let Some(times) = daily.get("time").and_then(|v| v.as_array()) else { return (None, None, None); };
    let Ok(now_signed) = i64::try_from(now) else { return (None, None, None); };
    let Some(index) = times.iter().position(|v| v.as_i64().is_some_and(|t| t > 0 && t.checked_add(offset).map(|local| local.div_euclid(86400)) == now_signed.checked_add(offset).map(|local| local.div_euclid(86400)))) else { return (None, None, None); };
    let day_start = times[index].as_u64().unwrap_or(0);
    let day_end = day_start.saturating_add(86400);
    let pair = array_epoch(daily, "sunrise", index).zip(array_epoch(daily, "sunset", index))
        .filter(|(rise, set)| *rise >= day_start && *rise < *set && *set <= day_end);
    let duration = array_number(daily, "daylight_duration", index)
        .filter(|v| *v >= 0.0 && *v <= 86400.0).map(|v| v.round() as u32);
    (pair.map(|p| p.0), pair.map(|p| p.1), duration)
}

#[derive(Deserialize)]
struct IpApiLocation {
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    country_name: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default)]
    ip: Option<String>,
    #[serde(default)]
    success: Option<bool>,
    #[serde(default)]
    error: Option<bool>,
}

// Provider refusal/rate limits survive network wake-ups; changing routes is
// not permission to exceed a provider's quota.
static PRIMARY_IP_COOLDOWN: AtomicU64 = AtomicU64::new(0);
static FALLBACK_IP_COOLDOWN: AtomicU64 = AtomicU64::new(0);
fn lookup_ip_provider(url: &str, cooldown: &AtomicU64) -> Result<IpLocation, String> {
    if cooldown.load(Ordering::Acquire) > now_unix() {
        return Err("IP provider is cooling down".into());
    }
    let response = http_agent().get(url).set("User-Agent", UA).call().map_err(|error| {
        if let ureq::Error::Status(status, response) = &error {
            if matches!(status, 403 | 429) {
                let retry = response.header("Retry-After").and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(if *status == 429 { 86400 } else { LOCATION_FOR }).clamp(60, 86400);
                cooldown.store(now_unix().saturating_add(retry), Ordering::Release);
            }
        }
        redact_query(&format!("IP provider request failed: {error}"))
    })?;
    let body = response.into_string().map_err(|error| format!("IP provider read failed: {error}"))?;
    let parsed: IpApiLocation = serde_json::from_str(&body)
        .map_err(|error| format!("IP provider parse failed: {error}"))?;
    ip_location_from_response(parsed)
}
fn ip_location_from_response(parsed: IpApiLocation) -> Result<IpLocation, String> {
    if parsed.success == Some(false) || parsed.error == Some(true) {
        return Err("IP provider declined the lookup".into());
    }
    let ip = parsed.ip.unwrap_or_default();
    if ip.parse::<std::net::IpAddr>().is_err() {
        return Err("IP provider carried no valid public address".into());
    }
    Ok(IpLocation { city: parsed.city.unwrap_or_default(), country: parsed.country_name.or(parsed.country).unwrap_or_default(),
        ip, resolved_at: now_unix(), stale: false })
}
fn lookup_ip_location() -> Result<IpLocation, String> {
    match lookup_ip_provider("https://ipapi.co/json/", &PRIMARY_IP_COOLDOWN) {
        Ok(ip) => Ok(ip),
        Err(primary) => {
            // Only network identity is requested, never Windows coordinates.
            lookup_ip_provider("https://ipwho.is/?fields=ip,success,city,country", &FALLBACK_IP_COOLDOWN)
                .map(|ip| { unidtk_api::log("micrographics: IP resolved via fallback ipwho.is"); ip })
                .map_err(|fallback| format!("{primary}; fallback: {fallback}"))
        }
    }
}

fn accepts_position_source(source: i32) -> bool {
    position_method(source).is_some()
}

fn position_method(source: i32) -> Option<&'static str> {
    match source {
        0 => Some("cellular"),
        1 => Some("satellite"),
        2 => Some("wifi"),
        3 => Some("ip"),
        5 => Some("default"),
        6 => Some("coarse"),
        _ => None,
    }
}

/// Start only on the foreground host UI thread. Waiting for the permission
/// answer runs in a bounded worker and never blocks that thread.
#[cfg(windows)]
pub fn request_location_access_on_ui(
    cancel: Arc<AtomicBool>,
    callback: impl FnOnce(&'static str) + Send + 'static,
) -> Result<(), String> {
    use windows::{
        Devices::Geolocation::{GeolocationAccessStatus, Geolocator},
        Foundation::AsyncStatus,
        Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    };
    let operation = Geolocator::RequestAccessAsync().map_err(|e| e.to_string())?;
    let cancellation = operation.clone();
    thread::Builder::new()
        .name("location-consent".into())
        .spawn(move || {
            if unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_err() {
                let _ = operation.Cancel();
                callback("unavailable");
                return;
            }
            let started = Instant::now();
            let status = loop {
                if cancel.load(Ordering::Acquire) || started.elapsed() >= Duration::from_secs(60) {
                    let _ = operation.Cancel();
                    break "unavailable";
                }
                match operation.Status() {
                    Ok(AsyncStatus::Started) => thread::sleep(Duration::from_millis(50)),
                    Ok(_) => {
                        break match operation.GetResults() {
                            Ok(GeolocationAccessStatus::Allowed) => "allowed",
                            Ok(GeolocationAccessStatus::Denied) => "denied",
                            _ => "unavailable",
                        }
                    }
                    Err(_) => break "unavailable",
                }
            };
            drop(operation);
            unsafe {
                RoUninitialize();
            }
            callback(status);
        })
        .map_err(|error| {
            let _ = cancellation.Cancel();
            format!("location consent worker failed: {error}")
        })?;
    Ok(())
}

#[cfg(not(windows))]
pub fn request_location_access_on_ui(
    _: Arc<AtomicBool>,
    _: impl FnOnce(&'static str) + Send + 'static,
) -> Result<(), String> {
    Err("Windows location permission is unavailable on this platform".into())
}

#[cfg(windows)]
fn device_location(
    stop: &AtomicBool,
    access: &AtomicBool,
    revision: &AtomicU32,
    expected: u32,
    vpn_active: &AtomicBool,
) -> Result<Location, LocationError> {
    use windows::{
        Devices::Geolocation::{Geolocator, PositionAccuracy},
        Foundation::{AsyncStatus, TimeSpan},
        Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED},
    };
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(LocationError::from)?;
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                RoUninitialize();
            }
        }
    }
    let _apartment = Apartment;
    let locator = Geolocator::new().map_err(LocationError::from)?;
    locator.SetDesiredAccuracy(PositionAccuracy::High).map_err(LocationError::from)?;
    let fast_timeout = Duration::from_secs(2);
    let operation = if let Ok(op) = locator.GetGeopositionAsyncWithAgeAndTimeout(
        TimeSpan { Duration: 15 * 60 * 10_000_000 },
        TimeSpan { Duration: fast_timeout.as_secs() as i64 * 10_000_000 },
    ) {
        let started = Instant::now();
        let mut fast_ok = false;
        while started.elapsed() < fast_timeout {
            if stop.load(Ordering::Acquire)
                || !access.load(Ordering::Acquire)
                || revision.load(Ordering::Acquire) != expected
            {
                let _ = op.Cancel();
                return Err("Windows positioning cancelled".into());
            }
            if let Ok(status) = op.Status() {
                if status == AsyncStatus::Completed {
                    fast_ok = true;
                    break;
                } else if status != AsyncStatus::Started {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
        if fast_ok {
            op
        } else {
            let _ = op.Cancel();
            locator.GetGeopositionAsyncWithAgeAndTimeout(
                TimeSpan { Duration: 0 },
                TimeSpan { Duration: GEO_TIMEOUT.as_secs() as i64 * 10_000_000 },
            ).map_err(LocationError::from)?
        }
    } else {
        locator.GetGeopositionAsyncWithAgeAndTimeout(
            TimeSpan { Duration: 0 },
            TimeSpan { Duration: GEO_TIMEOUT.as_secs() as i64 * 10_000_000 },
        ).map_err(LocationError::from)?
    };
    let started = Instant::now();
    loop {
        if stop.load(Ordering::Acquire)
            || !access.load(Ordering::Acquire)
            || revision.load(Ordering::Acquire) != expected
            || started.elapsed() >= GEO_TIMEOUT
        {
            let _ = operation.Cancel();
            return Err("Windows positioning cancelled or timed out".into());
        }
        if operation.Status().map_err(LocationError::from)? != AsyncStatus::Started {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let position = operation.GetResults().map_err(LocationError::from)?;
    let coordinate = position.Coordinate().map_err(LocationError::from)?;
    let source = coordinate.PositionSource().map_err(LocationError::from)?;
    let accuracy_m = coordinate.Accuracy().ok().filter(|value| value.is_finite() && *value >= 0.0);
    unidtk_api::log(format!("micrographics: Windows positioning source={} method={} accuracy_m={}", source.0, position_method(source.0).unwrap_or("unknown"), accuracy_m.map(|value| value.round().to_string()).unwrap_or_else(|| "unknown".into())));
    if !accepts_position_source(source.0) {
        return Err("Windows returned a position with unknown provenance".into());
    }
    if source.0 == 3 && vpn_active.load(Ordering::Acquire) {
        unidtk_api::log("micrographics: Windows position source is IP while VPN is active; ignoring IP-based position to avoid VPN gateway override");
        return Err(LocationError::Unavailable("Windows position source is IP while VPN is active".into()));
    }
    let point = coordinate
        .Point()
        .and_then(|p| p.Position())
        .map_err(LocationError::from)?;
    let location = Location {
        latitude: point.Latitude,
        longitude: point.Longitude,
        name: String::new(),
        method: position_method(source.0).map(str::to_string),
        accuracy_m,
        resolved_at: now_unix(),
    };
    if !location.is_plausible() {
        return Err("Windows returned invalid coordinates".into());
    }
    Ok(location)
}

#[cfg(not(windows))]
fn device_location(
    _: &AtomicBool,
    _: &AtomicBool,
    _: &AtomicU32,
    _: u32,
    _: &AtomicBool,
) -> Result<Location, LocationError> {
    Err("Windows device positioning is unavailable on this platform".into())
}

#[derive(Debug)]
enum LocationError {
    Denied(String),
    Unavailable(String),
}

impl From<&str> for LocationError {
    fn from(message: &str) -> Self {
        Self::Unavailable(message.into())
    }
}

impl std::fmt::Display for LocationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied(message) | Self::Unavailable(message) => f.write_str(message),
        }
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for LocationError {
    fn from(error: windows::core::Error) -> Self {
        // Preserve the typed HRESULT before rendering an error for the log.
        if error.code() == windows::Win32::Foundation::E_ACCESSDENIED {
            Self::Denied(error.to_string())
        } else {
            Self::Unavailable(error.to_string())
        }
    }
}

/// Transport errors quote the request URL, and the weather URL's query is the
/// device position. The log is readable through the read-only socket door, so
/// the query never reaches it.
fn redact_query(message: &str) -> String {
    message
        .split(' ')
        .map(|word| match word.find('?') {
            Some(at) => format!(
                "{}?…{}",
                &word[..at],
                if word.ends_with(':') { ":" } else { "" }
            ),
            None => word.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn fetch_snapshot(location: &Location) -> Result<WeatherSnapshot, String> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}\
         &current=temperature_2m,apparent_temperature,wind_speed_10m,is_day,weather_code\
         &hourly=precipitation_probability,precipitation\
         &daily=sunrise,sunset,daylight_duration\
         &timeformat=unixtime&timezone=auto&forecast_days=2",
        location.latitude, location.longitude
    );
    let response = http_agent().get(&url)
        .set("User-Agent", UA)
        .timeout(HTTP_TIMEOUT)
        .call()
        .map_err(|e| redact_query(&format!("open-meteo request failed: {e}")))?;
    let body = response
        .into_string()
        .map_err(|e| redact_query(&format!("open-meteo read failed: {e}")))?;
    let parsed: MeteoResponse =
        serde_json::from_str(&body).map_err(|e| format!("open-meteo parse failed: {e}"))?;
    snapshot_from_response(parsed, location, now_unix())
}

fn snapshot_from_response(
    parsed: MeteoResponse,
    location: &Location,
    observed_at: u64,
) -> Result<WeatherSnapshot, String> {
    let precipitation_forecast = precipitation_hours(&parsed.hourly, observed_at);
    let (sunrise_at, sunset_at, daylight_seconds) = daylight_values(&parsed.daily, &parsed.utc_offset_seconds, observed_at);
    let timezone = parsed.timezone.as_str().filter(|v| !v.is_empty() && v.len() <= 100 && v.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))).map(str::to_owned);
    let current = parsed
        .current
        .ok_or_else(|| "open-meteo answer carried no current conditions".to_string())?;

    let temp_c = current
        .temperature_2m
        .filter(|v| v.is_finite() && v.round() >= i32::MIN as f64 && v.round() <= i32::MAX as f64)
        .ok_or_else(|| "open-meteo answer carried no temperature".to_string())?
        .round() as i32;
    let feels_c = current
        .apparent_temperature
        .filter(|v| v.is_finite() && v.round() >= i32::MIN as f64 && v.round() <= i32::MAX as f64)
        .map(|v| v.round() as i32);

    Ok(WeatherSnapshot {
        temp_c: Some(temp_c),
        feels_c,
        wind_kmh: current
            .wind_speed_10m
            .filter(|v| v.is_finite() && *v >= 0.0 && v.round() <= u32::MAX as f64)
            .map(|v| v.round() as u32),
        code: current
            .weather_code
            .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 99.0 && v.fract() == 0.0)
            .map(|v| v as u8)
            .filter(|v| {
                matches!(
                    v,
                    0..=3
                        | 45
                        | 48
                        | 51
                        | 53
                        | 55
                        | 56
                        | 57
                        | 61
                        | 63
                        | 65
                        | 66
                        | 67
                        | 71
                        | 73
                        | 75
                        | 77
                        | 80
                        | 81
                        | 82
                        | 85
                        | 86
                        | 95
                        | 96
                        | 99
                )
            }),
        is_day: current
            .is_day
            .filter(|v| *v == 0.0 || *v == 1.0)
            .map(|v| v == 1.0),
        location: location.name.clone(),
        location_name_status: if location.name.is_empty() { "resolving" } else { "available" }.into(),
        location_name_provider: None,
        observed_at,
        stale: false,
        weather_available: true,
        location_source: Some("windows".into()),
        location_method: location.method.clone(),
        accuracy_m: location.accuracy_m,
        location_status: "available".into(),
        ip_location: None,
        precipitation_forecast,
        sunrise_at,
        sunset_at,
        daylight_seconds,
        timezone,
    })
}

// ─── Runtime state ─────────────────────────────────────────────────────────────

/// The part of the module a command handler needs: one read of one value.
///
/// Deliberately tiny and cloneable. The kernel registers a clone as managed
/// state, and `init` fills the same shared cell — the shape `vpn` and
/// `mp3-player` use. Anything richer here would have to be reached through a
/// global, and a global is exactly how a module ends up with two instances that
/// quietly disagree.
#[derive(Clone, Default)]
pub struct WeatherRuntime {
    state: Arc<Mutex<Option<Arc<WeatherState>>>>,
}

impl WeatherRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    fn publish(&self, state: Arc<WeatherState>) {
        if let Ok(mut guard) = self.state.lock() {
            *guard = Some(state);
        }
    }

    /// The current observation, with its staleness decided at read time so a
    /// cached value cannot claim to be fresh hours after it was taken.
    pub fn snapshot(&self) -> Option<WeatherSnapshot> {
        let state = {
            let guard = self.state.lock().ok()?;
            guard.clone()?
        };
        let guard = state.snapshot.lock().ok()?;
        let mut snapshot = guard.clone()?;
        state.mark_network_freshness(&mut snapshot);
        drop(guard);
        let age = now_unix().saturating_sub(snapshot.observed_at);
        snapshot.stale |= snapshot.temp_c.is_some() && age > FRESH_FOR;
        if let Some(ip) = snapshot.ip_location.as_mut() {
            ip.stale |= now_unix().saturating_sub(ip.resolved_at) > LOCATION_FOR;
        }
        Some(snapshot)
    }

    /// Called only after the foreground UI has received the actual Windows access result.
    pub fn set_location_access(&self, status: &str) -> Result<(), String> {
        if !matches!(status, "requesting" | "allowed" | "denied" | "unavailable") {
            return Err("invalid Windows location access result".into());
        }
        let state = self
            .state
            .lock()
            .map_err(|_| "weather state poisoned")?
            .clone()
            .ok_or("micrographics is not ready")?;
        // Serialize permission revocation with cache/publication. A completed
        // request from the previous generation cannot republish revoked data.
        let _publication = state
            .publication
            .lock()
            .map_err(|_| "weather publication poisoned")?;
        if state.stop.load(Ordering::Acquire) {
            return Err("micrographics is stopping".into());
        }
        if status == "requesting" {
            state.access_revision.fetch_add(1, Ordering::AcqRel);
            state.location_allowed.store(false, Ordering::Release);
            if let Ok(mut guard) = state.snapshot.lock() {
                guard
                    .get_or_insert_with(|| WeatherSnapshot::empty("permission-required"))
                    .location_status = "requesting".into();
            }
            state.emit();
            return Ok(());
        }
        // A cancelled/timed-out consent operation says nothing about permission.
        // Restore the last persisted decision and preserve the live observation.
        let allowed = status == "allowed" || (status == "unavailable" && state.persisted_access());
        state.access_revision.fetch_add(1, Ordering::AcqRel);
        state.location_allowed.store(allowed, Ordering::Release);
        if status != "unavailable" {
            if let Ok(mut decision) = state.access_decision.lock() { *decision = Some(allowed); }
        }
        state.wake.store(true, Ordering::Release);
        if let Ok(store) = state.store.lock() {
            if let Some(store) = store.as_ref() {
                if status != "unavailable" {
                    if let Err(error) =
                        store.kv_set(KEY_ACCESS, if allowed { b"allowed" } else { b"denied" })
                    {
                        unidtk_api::log(format!(
                            "micrographics: access cache write failed: {error}"
                        ));
                    }
                }
                if status == "denied" {
                    if let Err(error) = store.kv_remove(KEY_SNAPSHOT) {
                        unidtk_api::log(format!(
                            "micrographics: revoked weather cache removal failed: {error}"
                        ));
                    }
                }
            }
        }
        if let Ok(mut guard) = state.snapshot.lock() {
            let snapshot =
                guard.get_or_insert_with(|| WeatherSnapshot::empty("permission-required"));
            if status == "denied" {
                snapshot.clear_weather("denied");
            } else {
                snapshot.location_status = if snapshot.weather_available {
                    if allowed {
                        "retrying"
                    } else {
                        "unavailable"
                    }
                } else if allowed {
                    "locating"
                } else {
                    "unavailable"
                }
                .into();
            }
        }
        if status == "denied" {
            if let Ok(mut cache) = state.place_name.lock() {
                *cache = None;
            }
            if let Ok(mut fix) = state.observation_fix.lock() {
                *fix = None;
            }
        }
        state.emit();
        if let Ok(worker) = state.thread.lock() {
            if let Some(worker) = worker.as_ref() {
                worker.thread().unpark();
            }
        }
        Ok(())
    }

    fn halt(&self) {
        if let Ok(guard) = self.state.lock() {
            if let Some(state) = guard.as_ref() {
                state.stop();
            }
        }
    }
}

/// Everything the refresh thread owns for its lifetime.
struct WeatherState {
    store: Mutex<Option<Arc<dyn unidtk_api::ModuleDb>>>,
    snapshot: Mutex<Option<WeatherSnapshot>>,
    place_name: Mutex<Option<PlaceName>>,
    /// Fix belonging to the last successful weather observation. Kept only in
    /// memory, independent of city-name expiry, to detect actual movement.
    observation_fix: Mutex<Option<Location>>,
    /// Consecutive failed cycles. Drives the backoff; reset by any success.
    failures: AtomicU32,
    location_allowed: AtomicBool,
    access_revision: AtomicU32,
    wake: AtomicBool,
    network_revision: AtomicU32,
    ip_revision: AtomicU32,
    weather_revision: AtomicU32,
    name_revision: AtomicU32,
    vpn_active: AtomicBool,
    access_decision: Mutex<Option<bool>>,
    publication: Mutex<()>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl WeatherState {
    fn persisted_access(&self) -> bool {
        self.store
            .lock()
            .ok()
            .and_then(|store| {
                store
                    .as_ref()
                    .and_then(|store| store.kv_get(KEY_ACCESS).ok().flatten())
            })
            .as_deref()
            == Some(b"allowed")
    }

    /// Asks the refresh loop to end, **without waiting for it**.
    ///
    /// The loop can be inside a network call, and `ureq` has no cancellation, so
    /// joining here would block for as long as `HTTP_TIMEOUT` — several times the
    /// per-module shutdown budget the kernel grants (`Duration::from_millis(1500)`).
    /// Exceeding that budget does not make shutdown fail; it makes the user wait
    /// through it while the desktop freezes.
    ///
    /// So the flag is set and the join is handed to a detached thread: the worker
    /// still ends cleanly whenever it happens to be between requests, and a
    /// shutdown never waits on the network. The only work the worker can still do
    /// after this returns is a single `kv_set` of a complete observation, which is
    /// one redb transaction — killed mid-commit it leaves the key at its previous
    /// value, never a torn one.
    fn stop(&self) {
        // Do not wait for cache I/O or an event sink on the host thread. A
        // transaction already committing may finish; emit rechecks this flag.
        self.stop.store(true, Ordering::Release);
        let handle = match self.thread.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => None,
        };
        let Some(handle) = handle else { return };
        handle.thread().unpark();
        // A plain spawn, whose failure is deliberately ignored: dropping the
        // returned handle detaches the reaper, which is exactly the property that
        // matters — a detached join never blocks this thread either way.
        thread::Builder::new()
            .name("weather-reaper".to_string())
            .spawn(move || {
                let _ = handle.join();
            })
            .ok();
    }

    /// How long to wait before the next attempt: the cadence when healthy,
    /// stretched by each consecutive failure, capped at [`BACKOFF_MAX`].
    #[cfg(test)]
    fn next_delay(&self) -> Duration {
        let failures = self.failures.load(Ordering::Relaxed).min(8);
        if failures == 0 {
            return REFRESH;
        }
        retry_delay(failures)
    }

    fn emit(&self) {
        if self.stop.load(Ordering::Acquire) {
            return;
        }
        let snapshot = self.snapshot.lock().ok().and_then(|guard| {
            let mut snapshot = guard.clone()?;
            self.mark_network_freshness(&mut snapshot);
            Some(snapshot)
        });
        if let Some(mut snapshot) = snapshot {
            snapshot.stale |= snapshot.temp_c.is_some() && now_unix().saturating_sub(snapshot.observed_at) > FRESH_FOR;
            if let Some(ip) = snapshot.ip_location.as_mut() { ip.stale |= now_unix().saturating_sub(ip.resolved_at) > LOCATION_FOR; }
            self.mark_network_freshness(&mut snapshot);
            if !self.stop.load(Ordering::Acquire) { unidtk_api::emit("weather-update", serde_json::json!(snapshot)); }
        }
    }

    fn store_value(&self, key: &str, value: &impl Serialize) {
        let Ok(store) = self.store.lock() else { return };
        let Some(store) = store.as_ref() else { return };
        if let Ok(raw) = serde_json::to_vec(value) {
            if let Err(error) = store.kv_set(key, &raw) {
                unidtk_api::log(format!("micrographics: cache write failed: {error}"));
            }
        }
    }

    fn remember_snapshot(&self, snapshot: &WeatherSnapshot) {
        let Ok(store) = self.store.lock() else { return };
        let Some(store) = store.as_ref() else { return };
        match serde_json::to_vec(snapshot) {
            Ok(raw) => {
                if let Err(error) = store.kv_set(KEY_SNAPSHOT, &raw) {
                    unidtk_api::log(format!(
                        "micrographics: snapshot cache write failed: {error}"
                    ));
                }
            }
            Err(error) => {
                unidtk_api::log(format!("micrographics: snapshot encode failed: {error}"))
            }
        }
    }

    fn restore(&self) {
        let Ok(_publication) = self.publication.lock() else {
            return;
        };
        if self.stop.load(Ordering::Acquire) || self.access_revision.load(Ordering::Acquire) != 0 {
            return;
        }
        let Ok(store) = self.store.lock() else { return };
        let Some(store) = store.as_ref() else { return };
        // Legacy keys were produced from VPN/public-IP coordinates. Remove them
        // rather than letting an old observation masquerade as device weather.
        let _ = store.kv_remove("weather/snapshot");
        let _ = store.kv_remove("weather/location");
        let allowed = store.kv_get(KEY_ACCESS).ok().flatten().as_deref() == Some(b"allowed");
        self.location_allowed.store(allowed, Ordering::Release);
        let mut snapshot = if allowed {
            if let Some(raw) = store.kv_get(KEY_SNAPSHOT).ok().flatten() {
                if let Ok(mut cached) = serde_json::from_slice::<WeatherSnapshot>(&raw) {
                    cached.stale = true;
                    cached.location_status = "available".into();
                    cached
                } else {
                    WeatherSnapshot::empty("locating")
                }
            } else {
                WeatherSnapshot::empty("locating")
            }
        } else {
            WeatherSnapshot::empty("permission-required")
        };
        if snapshot.ip_location.is_none() {
            snapshot.ip_location = store
                .kv_get(KEY_IP)
                .ok()
                .flatten()
                .and_then(|raw| serde_json::from_slice(&raw).ok());
        }
        if let Some(raw) = store.kv_get("windows/place-fix-v1").ok().flatten() {
            if let Ok(cached_place) = serde_json::from_slice::<PlaceName>(&raw) {
                if let Ok(mut cache) = self.place_name.lock() {
                    *cache = Some(cached_place);
                }
            }
        }
        if let Ok(mut guard) = self.snapshot.lock() {
            *guard = Some(snapshot);
        }
    }

    fn mark_network_freshness(&self, snapshot: &mut WeatherSnapshot) {
        let network = self.network_revision.load(Ordering::Acquire);
        if let Some(ip) = snapshot.ip_location.as_mut() {
            ip.stale |= self.ip_revision.load(Ordering::Acquire) != network;
        }
        snapshot.stale |= snapshot.weather_available && self.weather_revision.load(Ordering::Acquire) != network;
        if self.name_revision.load(Ordering::Acquire) != network && snapshot.location_name_status == "available" {
            snapshot.stale = true;
        }
    }

    fn attach_store(&self, store: Arc<dyn unidtk_api::ModuleDb>) {
        let Ok(_publication) = self.publication.lock() else { return };
        if self.stop.load(Ordering::Acquire) { return; }
        let pending = self.access_decision.lock().ok().and_then(|value| *value);
        if let Some(allowed) = pending {
            // Consent may finish while the cache is still opening. Persist the
            // newer live decision before exposing the store to restoration.
            if let Err(error) = store.kv_set(KEY_ACCESS, if allowed { b"allowed" } else { b"denied" }) {
                unidtk_api::log(format!("micrographics: deferred access cache write failed: {error}"));
            }
            if !allowed { let _ = store.kv_remove(KEY_SNAPSHOT); }
        } else if self.access_revision.load(Ordering::Acquire) != 0 {
            let requesting = self.snapshot.lock().ok().and_then(|value| value.as_ref().map(|s| s.location_status == "requesting")).unwrap_or(false);
            if !requesting {
                let allowed = store.kv_get(KEY_ACCESS).ok().flatten().as_deref() == Some(b"allowed");
                self.location_allowed.store(allowed, Ordering::Release);
                self.wake.store(true, Ordering::Release);
            }
        }
        if let Ok(mut slot) = self.store.lock() { *slot = Some(store); }
    }

    fn network_changed(&self) {
        if self.stop.load(Ordering::Acquire) { return; }
        self.network_revision.fetch_add(1, Ordering::AcqRel);
        PRIMARY_IP_COOLDOWN.store(0, Ordering::Release);
        FALLBACK_IP_COOLDOWN.store(0, Ordering::Release);
        self.wake.store(true, Ordering::Release);
        if let Ok(worker) = self.thread.lock() {
            if let Some(worker) = worker.as_ref() { worker.thread().unpark(); }
        }
    }

    fn commit_ip(&self, network: u32, ip: IpLocation) -> bool {
        let Ok(_publication) = self.publication.lock() else { return false };
        if self.stop.load(Ordering::Acquire) || self.network_revision.load(Ordering::Acquire) != network { return false; }
        self.store_value(KEY_IP, &ip);
        // Cache I/O can overlap a route notification; do not advertise that
        // completed old request as the current network identity.
        if self.stop.load(Ordering::Acquire) || self.network_revision.load(Ordering::Acquire) != network { return false; }
        if let Ok(mut guard) = self.snapshot.lock() {
            self.ip_revision.store(network, Ordering::Release);
            guard.get_or_insert_with(|| WeatherSnapshot::empty("permission-required")).ip_location = Some(ip);
        }
        self.emit();
        true
    }

    fn refresh_once(&self, ip_clock: &mut RefreshSchedule, weather_clock: &mut RefreshSchedule,
        name_clock: &mut RefreshSchedule, fix: &mut Option<Location>) {
        let network = self.network_revision.load(Ordering::Acquire);
        if Instant::now() >= ip_clock.due {
            let result = lookup_ip_location();
            if self.stop.load(Ordering::Acquire) || self.network_revision.load(Ordering::Acquire) != network { return; }
            ip_clock.finish(result.is_ok());
            match result {
                Ok(ip) => { self.commit_ip(network, ip); }
                Err(error) => unidtk_api::log(format!("micrographics: IP indicator unavailable: {error}")),
            }
        }
        if self.stop.load(Ordering::Acquire) || !self.location_allowed.load(Ordering::Acquire) { return; }
        let revision = self.access_revision.load(Ordering::Acquire);
        if Instant::now() >= weather_clock.due {
            let location = match device_location(&self.stop, &self.location_allowed, &self.access_revision, revision, &self.vpn_active) {
                Ok(location) => location,
                Err(error) => {
                    self.record_location_failure(revision, &error);
                    weather_clock.finish(false);
                    unidtk_api::log(format!("micrographics: device location unavailable: {error}"));
                    return;
                }
            };
            if !self.can_publish_weather(revision) || self.network_revision.load(Ordering::Acquire) != network { return; }
            let Some(resolve_name) = self.prepare_place_name(revision, &location) else { return; };
            // Actual movement invalidates the name's healthy refresh deadline;
            // repeated weather failures alone do not defeat its retry backoff.
            let moved = fix.as_ref().is_some_and(|previous| !fixes_describe_same_place(previous, &location));
            if resolve_name && moved { name_clock.wake(); }
            // Reuse the fix for independent name retries, even when weather fails.
            *fix = Some(location.clone());
            if !resolve_name { self.name_revision.store(network, Ordering::Release); name_clock.finish(true); }
            let result = fetch_snapshot(&location);
            if !self.can_publish_weather(revision) || self.network_revision.load(Ordering::Acquire) != network { return; }
            weather_clock.finish(result.is_ok());
            match result {
                Ok(snapshot) => { self.commit_snapshot_on_network(revision, network, snapshot, Some(&location)); }
                Err(error) => {
                    self.record_weather_failure(revision);
                    unidtk_api::log(format!("micrographics: observation failed: {error}"));
                }
            }
        }
        if Instant::now() >= name_clock.due {
            if let Some(location) = fix.as_ref() {
                let result = lookup_place_name(location);
                if !self.can_publish_weather(revision) || self.network_revision.load(Ordering::Acquire) != network { return; }
                name_clock.finish(result.is_ok());
                if let Err(error) = &result { unidtk_api::log(format!("micrographics: {error}")); }
                self.commit_place_name_on_network(revision, network, location, result);
            }
        }
    }

    fn prepare_place_name(&self, revision: u32, fix: &Location) -> Option<bool> {
        let _publication = self.publication.lock().ok()?;
        if !self.can_publish_weather(revision) {
            return None;
        }
        let mut cache = self.place_name.lock().ok()?;
        // Whether this is the same place is decided by the uncertainty Windows
        // itself published, not by a fixed distance.
        let moved = self.observation_fix.lock().ok()?.as_ref().is_some_and(|previous| !fixes_describe_same_place(previous, fix));
        let cached = cache.as_ref().cloned();
        let fresh = cached.as_ref().filter(|cached| cached.matches(fix, now_unix())).cloned();
        // A name that still describes this place survives a failed refresh; a
        // name of another place, or a name nothing is known about, does not.
        let same_place = cached.as_ref().is_some_and(|cached| cached.describes_same_place(fix));
        if !same_place {
            *cache = None;
        }
        if let Ok(mut guard) = self.snapshot.lock() {
            let snapshot = guard.get_or_insert_with(|| WeatherSnapshot::empty("available"));
            if moved {
                // An observation from another place must not be presented under
                // a newly resolved city. TTL expiry alone never clears weather.
                snapshot.clear_weather("available");
            }
            snapshot.location_status = "available".into();
            snapshot.location_source = Some("windows".into());
            snapshot.location_method = fix.method.clone();
            snapshot.accuracy_m = fix.accuracy_m;
            match &fresh {
                Some(cached) => {
                    snapshot.location = cached.name.clone();
                    snapshot.location_name_status = "available".into();
                    snapshot.location_name_provider = cached.provider.clone();
                }
                None => {
                    if !same_place {
                        snapshot.location = String::new();
                        snapshot.location_name_provider = None;
                    }
                    snapshot.location_name_status = "resolving".into();
                }
            }
        }
        drop(cache);
        self.emit();
        Some(fresh.is_none())
    }

    #[cfg(test)]
    fn commit_place_name(&self, revision: u32, fix: &Location, result: Result<String, String>) -> bool {
        self.commit_place_name_on_network(revision, self.network_revision.load(Ordering::Acquire), fix,
            result.map(|name| (name, PlaceProvider::BigDataCloud)))
    }

    fn commit_place_name_on_network(&self, revision: u32, network: u32, fix: &Location, result: Result<(String, PlaceProvider), String>) -> bool {
        let Ok(_publication) = self.publication.lock() else { return false };
        if !self.can_publish_weather(revision) || self.network_revision.load(Ordering::Acquire) != network {
            return false;
        }
        let named = result.ok().filter(|(name, _)| !name.is_empty());
        if let Ok(mut cache) = self.place_name.lock() {
            // A failed lookup keeps the last good name: it stays cached until
            // it expires on its own terms, and stays on screen meanwhile.
            if let Some((name, provider)) = named.as_ref() {
                let place_obj = PlaceName {
                    latitude: fix.latitude,
                    longitude: fix.longitude,
                    name: name.clone(),
                    method: fix.method.clone(),
                    provider: Some(provider.id().to_string()),
                    resolved_at: now_unix(),
                };
                self.store_value("windows/place-fix-v1", &place_obj);
                *cache = Some(place_obj);
            }
        }
        if let Ok(mut guard) = self.snapshot.lock() {
            let snapshot = guard.get_or_insert_with(|| WeatherSnapshot::empty("available"));
            self.name_revision.store(network, Ordering::Release);
            match named {
                Some((name, provider)) => {
                    snapshot.location = name;
                    snapshot.location_name_status = "available".into();
                    snapshot.location_name_provider = Some(provider.id().to_string());
                }
                None => {
                    // The name that is already on screen stays there: a geocoder
                    // that is momentarily unreachable must not erase what it knew.
                    snapshot.location_name_status = "unavailable".into();
                }
            }
        }
        if let Some(snapshot) = self.snapshot.lock().ok().and_then(|s| s.clone()) {
            self.remember_snapshot(&snapshot);
        }
        self.emit();
        true
    }

    fn can_publish_weather(&self, revision: u32) -> bool {
        !self.stop.load(Ordering::Acquire)
            && self.location_allowed.load(Ordering::Acquire)
            && self.access_revision.load(Ordering::Acquire) == revision
    }

    fn record_location_failure(&self, revision: u32, error: &LocationError) {
        let Ok(_publication) = self.publication.lock() else {
            return;
        };
        if !self.can_publish_weather(revision) {
            return;
        }
        if matches!(error, LocationError::Denied(_)) {
            self.access_revision.fetch_add(1, Ordering::AcqRel);
            self.location_allowed.store(false, Ordering::Release);
            if let Ok(mut decision) = self.access_decision.lock() { *decision = Some(false); }
            if let Ok(mut cache) = self.place_name.lock() {
                *cache = None;
            }
            if let Ok(mut fix) = self.observation_fix.lock() {
                *fix = None;
            }
            if let Ok(store) = self.store.lock() {
                if let Some(store) = store.as_ref() {
                    if let Err(error) = store.kv_set(KEY_ACCESS, b"denied") {
                        unidtk_api::log(format!(
                            "micrographics: access cache write failed: {error}"
                        ));
                    }
                    if let Err(error) = store.kv_remove(KEY_SNAPSHOT) {
                        unidtk_api::log(format!(
                            "micrographics: revoked weather cache removal failed: {error}"
                        ));
                    }
                }
            }
            if let Ok(mut guard) = self.snapshot.lock() {
                guard
                    .get_or_insert_with(|| WeatherSnapshot::empty("denied"))
                    .clear_weather("denied");
            }
        } else if let Ok(mut guard) = self.snapshot.lock() {
            let snapshot = guard.get_or_insert_with(|| WeatherSnapshot::empty("unavailable"));
            snapshot.location_status = if snapshot.weather_available {
                "retrying"
            } else {
                "unavailable"
            }
            .into();
        }
        self.failures.fetch_add(1, Ordering::Relaxed);
        self.emit();
    }

    fn record_weather_failure(&self, revision: u32) {
        let Ok(_publication) = self.publication.lock() else {
            return;
        };
        if !self.can_publish_weather(revision) {
            return;
        }
        self.failures.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.snapshot.lock() {
            let snapshot = guard.get_or_insert_with(|| WeatherSnapshot::empty("available"));
            snapshot.location_status = "available".into();
            snapshot.location_source = Some("windows".into());
            snapshot.stale |= snapshot.temp_c.is_some()
                && now_unix().saturating_sub(snapshot.observed_at) > FRESH_FOR;
            snapshot.weather_available = snapshot.temp_c.is_some();
        }
        self.emit();
    }

    #[cfg(test)]
    fn commit_snapshot(&self, revision: u32, snapshot: WeatherSnapshot) -> bool {
        self.commit_snapshot_at(revision, snapshot, None)
    }

    #[cfg(test)]
    fn commit_snapshot_at(&self, revision: u32, snapshot: WeatherSnapshot, fix: Option<&Location>) -> bool {
        self.commit_snapshot_on_network(revision, self.network_revision.load(Ordering::Acquire), snapshot, fix)
    }

    fn commit_snapshot_on_network(&self, revision: u32, network: u32, mut snapshot: WeatherSnapshot, fix: Option<&Location>) -> bool {
        let Ok(_publication) = self.publication.lock() else {
            return false;
        };
        if !self.can_publish_weather(revision) || self.network_revision.load(Ordering::Acquire) != network {
            return false;
        }
        self.failures.store(0, Ordering::Relaxed);
        if let Some(fix) = fix {
            if let Ok(mut previous) = self.observation_fix.lock() {
                *previous = Some(fix.clone());
            }
        }
        if let Ok(mut guard) = self.snapshot.lock() {
            self.weather_revision.store(network, Ordering::Release);
            snapshot.ip_location = guard.as_ref().and_then(|s| s.ip_location.clone());
            if let Some(previous) = guard.as_ref() {
                snapshot.location = previous.location.clone();
                snapshot.location_name_status = previous.location_name_status.clone();
                snapshot.location_name_provider = previous.location_name_provider.clone();
            }
            *guard = Some(snapshot.clone());
        }
        self.remember_snapshot(&snapshot);
        self.emit();
        true
    }

    /// Returns at the deadline, on a permission wakeup, or on shutdown.
    fn wait_delay(&self, delay: Duration) {
        let started = Instant::now();
        loop {
            if self.stop.load(Ordering::Acquire) || self.wake.swap(false, Ordering::AcqRel) {
                return;
            }
            let remaining = delay.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return;
            }
            // All producers unpark on wake/stop; steady state needs no polling.
            thread::park_timeout(remaining);
        }
    }

    fn run(&self) {
        self.restore();
        self.emit();
        let mut ip_clock = RefreshSchedule::new(Duration::from_secs(LOCATION_FOR));
        let mut weather_clock = RefreshSchedule::new(REFRESH);
        let mut name_clock = RefreshSchedule::new(Duration::from_secs(NAME_FOR));
        let mut fix = None;
        let mut network = self.network_revision.load(Ordering::Acquire);
        let mut access = self.access_revision.load(Ordering::Acquire);
        while !self.stop.load(Ordering::Acquire) {
            let current_network = self.network_revision.load(Ordering::Acquire);
            let current_access = self.access_revision.load(Ordering::Acquire);
            if current_network != network || current_access != access {
                ip_clock.wake(); weather_clock.wake(); name_clock.wake(); fix = None;
                network = current_network; access = current_access;
                self.wake.swap(false, Ordering::AcqRel);
                self.wait_delay(Duration::from_secs(1));
                continue;
            }
            self.refresh_once(&mut ip_clock, &mut weather_clock, &mut name_clock, &mut fix);
            let mut due = ip_clock.due;
            if self.location_allowed.load(Ordering::Acquire) {
                due = due.min(weather_clock.due);
                if fix.is_some() { due = due.min(name_clock.due); }
            }
            self.wait_delay(due.saturating_duration_since(Instant::now()));
        }
    }

}

/// The module half: the kernel initialises it, and it owns the lifecycle.
pub struct MicrographicsModule {
    runtime: WeatherRuntime,
}

impl MicrographicsModule {
    pub fn new() -> Self {
        Self {
            runtime: WeatherRuntime::new(),
        }
    }

    /// The state half, for the kernel to register as managed state. A clone, so
    /// what a command handler reaches and what `init` filled are one value.
    pub fn runtime(&self) -> WeatherRuntime {
        self.runtime.clone()
    }
}

impl Default for MicrographicsModule {
    fn default() -> Self {
        Self::new()
    }
}

pub fn create_module() -> Box<dyn unidtk_api::Module> {
    Box::new(MicrographicsModule::new())
}

/// The live state a command handler was handed, or a clear reason there is none.
///
/// Reached the way `vpn` reaches its runtime: the host handle is downcast to the
/// app handle and the registered state is looked up there. A missing runtime is a
/// wiring bug, and says so rather than reading as "no weather".
fn runtime_from<'a>(
    host: Option<unidtk_api::commands::HostHandle<'a>>,
) -> Result<tauri::State<'a, WeatherRuntime>, String> {
    let app = host
        .and_then(|handle| handle.downcast_ref::<tauri::AppHandle>())
        .ok_or_else(|| "host handle unavailable".to_string())?;
    tauri::Manager::try_state::<WeatherRuntime>(app)
        .ok_or_else(|| "micrographics is not ready".to_string())
}

/// Weather and independent network identity, including permission state.
///
/// Deliberately says "no observation yet" rather than a zeroed reading: a
/// fabricated `0°` is a lie the panel would have no way to detect.
pub fn weather_current(
    host: Option<unidtk_api::commands::HostHandle<'_>>,
) -> Result<WeatherSnapshot, String> {
    runtime_from(host)?
        .snapshot()
        .ok_or_else(|| "weather: no observation yet".to_string())
}

/// Answers the socket commands this module owns.
///
/// The name must equal the manifest's: the kernel addresses a module by the name
/// it read from the inventory, and a mismatch reads as "declared but has no
/// handler".
pub fn ipc_call(
    cmd: &str,
    _args: &serde_json::Value,
    host: Option<unidtk_api::commands::HostHandle<'_>>,
) -> Option<Result<serde_json::Value, String>> {
    match cmd {
        "weather_current" => Some(
            weather_current(host)
                .and_then(|snapshot| serde_json::to_value(snapshot).map_err(|e| e.to_string())),
        ),
        _ => None,
    }
}

impl unidtk_api::Module for MicrographicsModule {
    fn id(&self) -> &'static str {
        "micrographics"
    }

    fn dependencies(&self) -> &'static [unidtk_api::manifest::ModuleDependency] {
        &[]
    }

    fn init(&self, ctx: unidtk_api::ModuleContext) -> Result<(), String> {
        if ctx.is_diagnostic() {
            // Diagnostics must not start an endless background thread. Report
            // whether a position is on disk instead — that is the part a doctor
            // can meaningfully check without touching the network.
            let cached = match ctx.store(unidtk_api::StoreSpec::cache("weather")) {
                Ok(store) => store.kv_get(KEY_ACCESS).ok().flatten().is_some(),
                Err(_) => false,
            };
            unidtk_api::log(format!(
                "micrographics: diagnostic mode, cached position present: {cached}"
            ));
            return Ok(());
        }
        if !ctx.enabled() {
            unidtk_api::log("micrographics: disabled — weather refresh not started");
            return Ok(());
        }

        let state = Arc::new(WeatherState {
            store: Mutex::new(None),
            snapshot: Mutex::new(Some(WeatherSnapshot::empty("permission-required"))),
            place_name: Mutex::new(None),
            observation_fix: Mutex::new(None),
            failures: AtomicU32::new(0),
            location_allowed: AtomicBool::new(false),
            access_revision: AtomicU32::new(0),
            wake: AtomicBool::new(false),
            network_revision: AtomicU32::new(0),
            ip_revision: AtomicU32::new(0),
            weather_revision: AtomicU32::new(0),
            name_revision: AtomicU32::new(0),
            vpn_active: AtomicBool::new(false),
            access_decision: Mutex::new(None),
            publication: Mutex::new(()),
            stop: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        });

        self.runtime.publish(Arc::clone(&state));
        for event in ["network-changed", "system-resumed"] {
            let weak = Arc::downgrade(&state);
            unidtk_api::subscribe(event, move |_| {
                if let Some(state) = weak.upgrade() { state.network_changed(); }
            });
        }
        let weak = Arc::downgrade(&state);
        // VPN health/latency updates are not route changes.
        let vpn_route = Arc::new(Mutex::new(None));
        unidtk_api::subscribe("vpn", move |payload| {
            let connected = payload.get("connected").and_then(|v| v.as_bool()).unwrap_or(false);
            let route = (connected,
                payload.get("selected_tag").and_then(|v| v.as_str()).map(str::to_string));
            let changed = if let Ok(mut previous) = vpn_route.lock() {
                let changed = previous.as_ref() != Some(&route);
                *previous = Some(route); changed
            } else { false };
            if let Some(state) = weak.upgrade() {
                state.vpn_active.store(connected, Ordering::Release);
                if changed { state.network_changed(); }
            }
        });
        let worker = Arc::clone(&state);
        let mut thread_slot = state.thread.lock().map_err(|_| "weather state poisoned")?;
        let handle = thread::Builder::new()
            .name("weather".to_string())
            .spawn(move || {
                if worker.stop.load(Ordering::Acquire) { return; }
                match ctx.store(unidtk_api::StoreSpec::cache("weather")) {
                    Ok(store) => {
                        if worker.stop.load(Ordering::Acquire) { return; }
                        worker.attach_store(store);
                    }
                    Err(error) => unidtk_api::log(format!("micrographics: cache unavailable: {error}")),
                }
                if !worker.stop.load(Ordering::Acquire) { worker.run(); }
            })
            .map_err(|e| format!("weather thread spawn failed: {e}"))?;
        *thread_slot = Some(handle);
        drop(thread_slot);
        unidtk_api::log("micrographics: weather refresh started");
        Ok(())
    }

    fn shutdown(&self, _budget: std::time::Duration) -> Result<(), String> {
        self.runtime.halt();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    fn device_fix() -> Location {
        Location {
            latitude: 55.7558,
            longitude: 37.6173,
            name: String::new(),
            method: Some("wifi".into()),
            accuracy_m: Some(100.0),
            resolved_at: now_unix(),
        }
    }

    fn place_response(source: &str) -> PlaceResponse {
        PlaceResponse {
            latitude: Some(55.7558),
            longitude: Some(37.6173),
            lookup_source: Some(source.into()),
            city: Some("Москва".into()),
            locality: None,
            principal_subdivision: None,
        }
    }

    #[test]
    fn ip_provider_formats_and_declined_results_are_validated() {
        let primary: IpApiLocation = serde_json::from_str(r#"{"ip":"8.8.8.8","city":"Example","country":"US","country_name":"Country"}"#).unwrap();
        assert_eq!(ip_location_from_response(primary).unwrap().country, "Country");
        let fallback: IpApiLocation = serde_json::from_str(r#"{"ip":"2001:4860:4860::8888","success":true,"city":"Example","country":"Country","latitude":0,"longitude":0}"#).unwrap();
        let ip = ip_location_from_response(fallback).unwrap();
        assert_eq!(ip.country, "Country");
        assert!(!ip.stale);
        for raw in [r#"{"ip":"8.8.8.8","success":false}"#, r#"{"ip":"8.8.8.8","error":true}"#, r#"{"ip":"invalid","success":true}"#] {
            assert!(ip_location_from_response(serde_json::from_str(raw).unwrap()).is_err());
        }
    }

    #[test]
    fn place_name_requires_coordinate_source_and_matching_echo() {
        let fix = device_fix();
        for source in ["coordinates", "reverseGeocoding"] {
            assert_eq!(place_name_from_response(place_response(source), &fix).unwrap(), "Москва");
        }
        for source in ["ipAddress", "ip", "", "unknown"] {
            assert!(place_name_from_response(place_response(source), &fix).is_err());
        }
        let mut mismatch = place_response("coordinates");
        mismatch.latitude = Some(38.75);
        assert!(place_name_from_response(mismatch, &fix).is_err());
        let mut missing = place_response("coordinates");
        missing.longitude = None;
        assert!(place_name_from_response(missing, &fix).is_err());
    }

    #[test]
    fn place_name_validates_values_and_falls_back_only_to_locality_or_region() {
        let fix = device_fix();
        let mut response = place_response("coordinates");
        response.city = Some("  ".into());
        response.locality = Some("  Химки  ".into());
        assert_eq!(place_name_from_response(response, &fix).unwrap(), "Химки");
        let mut response = place_response("coordinates");
        response.city = None;
        response.principal_subdivision = Some("Московская область".into());
        assert_eq!(place_name_from_response(response, &fix).unwrap(), "Московская область");
        let mut response = place_response("coordinates");
        response.latitude = Some(f64::NAN);
        assert!(place_name_from_response(response, &fix).is_err());
        let mut response = place_response("coordinates");
        response.city = Some("invalid\ncity".into());
        assert!(place_name_from_response(response, &fix).is_err());
        let empty: PlaceResponse = serde_json::from_str("{}").unwrap();
        assert!(place_name_from_response(empty, &fix).is_err());
    }

    fn nominatim_response(bounding_box: [&str; 4], address: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "lat": "55.7553057",
            "lon": "37.6181590",
            "boundingbox": bounding_box,
            "address": address,
        })
    }

    fn reset_place_cooldowns() {
        BIGDATACLOUD_COOLDOWN.store(0, Ordering::Release);
        OPENSTREETMAP_COOLDOWN.store(0, Ordering::Release);
    }

    #[test]
    fn nominatim_name_requires_the_queried_point_inside_the_bounding_box() {
        let fix = device_fix();
        let inside = nominatim_response(
            ["55.70", "55.80", "37.60", "37.70"],
            serde_json::json!({"city": "Москва"}),
        );
        assert_eq!(nominatim_place_name(&inside, &fix).unwrap(), "Москва");
        // The answer carries the centroid of the matched feature, never an echo
        // of the query: only the bounding box ties it to the place.
        let elsewhere = Location { latitude: 60.0, ..device_fix() };
        assert!(nominatim_place_name(&inside, &elsewhere).is_err());
        let mut refused = inside.clone();
        refused["error"] = serde_json::json!("Unable to geocode");
        assert!(nominatim_place_name(&refused, &fix).is_err());
        let mut without_box = inside.clone();
        without_box.as_object_mut().unwrap().remove("boundingbox");
        assert!(nominatim_place_name(&without_box, &fix).is_err());
        let mut without_address = inside.clone();
        without_address.as_object_mut().unwrap().remove("address");
        assert!(nominatim_place_name(&without_address, &fix).is_err());
        let mut malformed = inside.clone();
        malformed["boundingbox"] = serde_json::json!(["55.70", "55.80"]);
        assert!(nominatim_place_name(&malformed, &fix).is_err());
        // The nearest settlement first, the region last.
        let ranked = nominatim_response(
            ["55.70", "55.80", "37.60", "37.70"],
            serde_json::json!({"state": "Республика", "municipality": "Сельсовет", "town": "Посёлок"}),
        );
        assert_eq!(nominatim_place_name(&ranked, &fix).unwrap(), "Посёлок");
    }

    #[test]
    fn place_lookup_falls_back_to_the_second_provider() {
        reset_place_cooldowns();
        let fix = device_fix();
        let mut asked: Vec<PlaceProvider> = Vec::new();
        let result = lookup_place_name_with(
            &[PlaceProvider::BigDataCloud, PlaceProvider::OpenStreetMap],
            &fix,
            |provider, _| {
                asked.push(provider);
                match provider {
                    PlaceProvider::BigDataCloud => Err(PlaceLookupError::Unreachable("blocked".into())),
                    _ => Ok("Москва".into()),
                }
            },
        );
        assert_eq!(asked, vec![PlaceProvider::BigDataCloud, PlaceProvider::OpenStreetMap]);
        assert_eq!(result.unwrap(), ("Москва".into(), PlaceProvider::OpenStreetMap));
        reset_place_cooldowns();
    }

    #[test]
    fn a_refused_provider_is_left_alone_while_the_chain_continues() {
        reset_place_cooldowns();
        let fix = device_fix();
        let result = lookup_place_name_with(
            &[PlaceProvider::BigDataCloud, PlaceProvider::OpenStreetMap],
            &fix,
            |provider, _| match provider {
                PlaceProvider::BigDataCloud => Err(PlaceLookupError::Refused("refused".into())),
                _ => Ok("Москва".into()),
            },
        );
        assert_eq!(result.unwrap().1, PlaceProvider::OpenStreetMap);
        assert!(BIGDATACLOUD_COOLDOWN.load(Ordering::Acquire) > now_unix());
        // A provider that refused the client is not asked again while it cools.
        let mut asked = 0;
        let _ = lookup_place_name_with(
            &[PlaceProvider::BigDataCloud],
            &fix,
            |_, _| { asked += 1; Ok("Москва".into()) },
        );
        assert_eq!(asked, 0);
        reset_place_cooldowns();
    }

    #[test]
    fn the_last_good_name_survives_a_failed_refresh() {
        let state = state_with(None);
        state.location_allowed.store(true, Ordering::Release);
        let fix = device_fix();
        state.prepare_place_name(0, &fix);
        state.commit_place_name(0, &fix, Ok("Москва".into()));
        // The name expires, but the place did not change: the name stays.
        state.place_name.lock().unwrap().as_mut().unwrap().resolved_at =
            now_unix().saturating_sub(NAME_FOR + 1);
        assert_eq!(state.prepare_place_name(0, &fix), Some(true));
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.location, "Москва");
        // A provider that cannot be reached does not erase it either.
        state.commit_place_name(0, &fix, Err("blocked".into()));
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.location, "Москва");
        assert_eq!(snapshot.location_name_status, "unavailable");
    }

    #[test]
    fn a_network_estimate_keeps_its_name_within_the_accuracy_windows_claims() {
        let state = state_with(None);
        state.location_allowed.store(true, Ordering::Release);
        // Windows estimates the position from the network: 1500 m of uncertainty.
        let fix = Location { method: Some("ip".into()), accuracy_m: Some(1500.0), ..device_fix() };
        state.prepare_place_name(0, &fix);
        state.commit_place_name(0, &fix, Ok("Москва".into()));
        // A kilometre and a half of drift is inside the claimed uncertainty.
        let drift = Location { latitude: fix.latitude + 0.01, ..fix.clone() };
        assert_eq!(state.prepare_place_name(0, &drift), Some(false));
        assert_eq!(state.snapshot.lock().unwrap().as_ref().unwrap().location, "Москва");
        // Two kilometres are not: that is another place, and the name goes.
        let away = Location { latitude: fix.latitude + 0.02, ..fix.clone() };
        assert_eq!(state.prepare_place_name(0, &away), Some(true));
        assert_eq!(state.snapshot.lock().unwrap().as_ref().unwrap().location, "");
    }

    #[test]
    fn the_snapshot_carries_who_named_the_place() {
        let state = state_with(None);
        state.location_allowed.store(true, Ordering::Release);
        let fix = device_fix();
        state.prepare_place_name(0, &fix);
        assert!(state.commit_place_name_on_network(
            0,
            state.network_revision.load(Ordering::Acquire),
            &fix,
            Ok(("Москва".into(), PlaceProvider::OpenStreetMap)),
        ));
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.location_name_provider.as_deref(), Some("openstreetmap"));
        let raw = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(raw["locationNameProvider"], "openstreetmap");
    }

    #[test]
    fn late_place_response_cannot_publish_after_revision_changes() {
        let state = state_with(Some(snapshot_at(now_unix())));
        state.location_allowed.store(true, Ordering::Release);
        let fix = device_fix();
        assert_eq!(state.prepare_place_name(0, &fix), Some(true));
        state.access_revision.store(1, Ordering::Release);
        assert!(!state.commit_place_name(0, &fix, Ok("Old city".into())));
        assert!(state.place_name.lock().unwrap().is_none());
        assert_eq!(state.snapshot.lock().unwrap().as_ref().unwrap().location, "");
    }

    #[test]
    fn naming_never_changes_weather_availability_and_survives_weather_failure() {
        let state = state_with(Some(snapshot_at(now_unix())));
        state.location_allowed.store(true, Ordering::Release);
        let fix = device_fix();
        state.prepare_place_name(0, &fix);
        assert!(state.commit_place_name(0, &fix, Ok("Москва".into())));
        assert!(state.commit_snapshot(0, snapshot_at(now_unix())));
        state.record_weather_failure(0);
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.location, "Москва");
        assert_eq!(snapshot.location_name_status, "available");
        assert_eq!(snapshot.temp_c, Some(7));
        assert!(snapshot.weather_available);
        assert!(state.commit_place_name(0, &fix, Err("timeout".into())));
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.temp_c, Some(7));
        assert!(snapshot.weather_available);
        assert_eq!(snapshot.location_name_status, "unavailable");
        let empty = state_with(Some(WeatherSnapshot::empty("locating")));
        empty.location_allowed.store(true, Ordering::Release);
        empty.prepare_place_name(0, &fix);
        empty.record_weather_failure(0);
        assert!(empty.commit_place_name(0, &fix, Ok("Москва".into())));
        let snapshot = empty.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.location, "Москва");
        assert!(!snapshot.weather_available);
    }

    #[test]
    fn name_cache_requires_same_fresh_fix_and_expires_in_memory() {
        let state = state_with(None);
        state.location_allowed.store(true, Ordering::Release);
        let fix = device_fix();
        state.prepare_place_name(0, &fix);
        state.commit_place_name(0, &fix, Ok("Москва".into()));
        let near = Location { latitude: fix.latitude + 0.003, ..device_fix() };
        assert_eq!(state.prepare_place_name(0, &near), Some(false));
        assert_eq!(state.snapshot.lock().unwrap().as_ref().unwrap().location, "Москва");
        let far = Location { latitude: fix.latitude + 0.02, ..device_fix() };
        assert_eq!(state.prepare_place_name(0, &far), Some(true));
        assert_eq!(state.snapshot.lock().unwrap().as_ref().unwrap().location, "");
        assert!(state.place_name.lock().unwrap().is_none());
        state.commit_place_name(0, &fix, Ok("Москва".into()));
        state.place_name.lock().unwrap().as_mut().unwrap().resolved_at = now_unix().saturating_sub(NAME_FOR + 1);
        assert_eq!(state.prepare_place_name(0, &fix), Some(true));
    }

    #[test]
    fn real_movement_invalidates_previous_weather_but_name_ttl_expiry_does_not() {
        let state = state_with(None);
        state.location_allowed.store(true, Ordering::Release);
        let fix = device_fix();
        state.prepare_place_name(0, &fix);
        state.commit_snapshot_at(0, snapshot_at(now_unix()), Some(&fix));
        state.commit_place_name(0, &fix, Ok("Москва".into()));
        state.place_name.lock().unwrap().as_mut().unwrap().resolved_at = now_unix().saturating_sub(NAME_FOR + 1);
        assert_eq!(state.prepare_place_name(0, &fix), Some(true));
        assert!(state.snapshot.lock().unwrap().as_ref().unwrap().weather_available);
        let far = Location { latitude: fix.latitude + 0.02, ..device_fix() };
        assert_eq!(state.prepare_place_name(0, &far), Some(true));
        state.record_weather_failure(0);
        state.commit_place_name(0, &far, Ok("Другой город".into()));
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.location, "Другой город");
        assert_eq!(snapshot.location_name_status, "available");
        assert_eq!(snapshot.location_source.as_deref(), Some("windows"));
        let raw = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(raw["locationNameStatus"], "available");
        assert_eq!(raw["locationSource"], "windows");
        assert!(!snapshot.weather_available);
        assert_eq!(snapshot.temp_c, None);
        state.commit_snapshot_at(0, snapshot_at(now_unix()), Some(&far));
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.location, "Другой город");
        assert!(snapshot.weather_available);
        assert_eq!(snapshot.temp_c, Some(7));
    }

    #[test]
    fn old_snapshot_without_name_status_deserializes_and_revocation_clears_name_cache() {
        let mut raw = serde_json::to_value(snapshot_at(now_unix())).unwrap();
        raw.as_object_mut().unwrap().remove("locationNameStatus");
        let old: WeatherSnapshot = serde_json::from_value(raw).unwrap();
        assert_eq!(old.location_name_status, "unavailable");
        let state = state_with(Some(old));
        state.location_allowed.store(true, Ordering::Release);
        let fix = device_fix();
        state.commit_place_name(0, &fix, Ok("Москва".into()));
        state.record_location_failure(0, &LocationError::Denied("denied".into()));
        assert!(state.place_name.lock().unwrap().is_none());
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert!(snapshot.location.is_empty());
        assert_eq!(snapshot.location_name_status, "unavailable");
    }

    use super::*;

    fn provider_snapshot(raw: &str) -> Result<WeatherSnapshot, String> {
        snapshot_from_response(
            serde_json::from_str(raw).unwrap(),
            &Location {
                latitude: 55.0,
                longitude: 37.0,
                name: String::new(),
                method: Some("wifi".into()),
                accuracy_m: Some(100.0),
                resolved_at: 100,
            },
            100,
        )
    }

    #[test]
    fn precipitation_uses_future_endpoints_and_preserves_unknown_and_zero() {
        let hourly = serde_json::json!({"time":[3600,7200,10800,14400],"precipitation_probability":[99,0,null,45],"precipitation":[9,0,"missing",1.25]});
        let hours = precipitation_hours(&hourly, 3600);
        assert_eq!(hours.len(), 3);
        assert_eq!(hours[0].at, 7200);
        assert_eq!(hours[0].probability, Some(0));
        assert_eq!(hours[0].amount_mm, Some(0.0));
        assert_eq!(hours[1].probability, None);
        assert_eq!(hours[1].amount_mm, None);
        assert_eq!(hours[2].amount_mm, Some(1.25));
        assert_eq!(precipitation_hours(&hourly, 3601)[0].at, 7200);
        assert!(precipitation_hours(&hourly, 14400).is_empty());
    }

    #[test]
    fn precipitation_rejects_gaps_but_invalid_values_stay_unknown() {
        assert!(precipitation_hours(&serde_json::json!({"time":[7200,14400,18000]}), 4000).is_empty());
        let hourly = serde_json::json!({"time":[7200,10800,14400],"precipitation_probability":[101,-1,0.5],"precipitation":[-1,1001,{}]});
        let hours = precipitation_hours(&hourly, 4000);
        assert_eq!(hours.len(), 3);
        assert!(hours.iter().all(|h| h.probability.is_none() && h.amount_mm.is_none()));
    }

    #[test]
    fn precipitation_retains_one_reserve_hour_without_requiring_it() {
        let hourly = serde_json::json!({"time":[7200,10800,14400,18000,21600],"precipitation_probability":[0,10,20,30,40],"precipitation":[0,0.1,0.2,0.3,0.4]});
        let hours = precipitation_hours(&hourly, 4000);
        assert_eq!(hours.len(), 4);
        assert_eq!(hours[3].at, 18000);
        assert_eq!(hours[3].probability, Some(30));
        assert_eq!(hours.iter().filter(|h| h.at > 7201).count(), 3);
        for reserve in [serde_json::Value::Null, serde_json::json!("bad"), serde_json::json!(21600)] {
            let mut broken = hourly.clone();
            broken["time"][3] = reserve;
            assert_eq!(precipitation_hours(&broken, 4000), hours[..3].to_vec());
        }
        let missing = serde_json::json!({"time":[7200,10800,14400]});
        assert_eq!(precipitation_hours(&missing, 4000).len(), 3);
    }

    #[test]
    fn daylight_selects_location_date_across_utc_midnight() {
        // UTC+3 local midnight is the preceding UTC day's 21:00.
        let daily = serde_json::json!({"time":[162000,248400],"sunrise":[183600,270000],"sunset":[226800,313200],"daylight_duration":[43200,43200]});
        assert_eq!(daylight_values(&daily, &serde_json::json!(10800), 167000), (Some(183600), Some(226800), Some(43200)));
        assert_eq!(daylight_values(&daily, &serde_json::json!(10800), 249000), (Some(270000), Some(313200), Some(43200)));
        assert_eq!(daylight_values(&daily, &serde_json::json!(10800), 400000), (None, None, None));
    }

    #[test]
    fn polar_and_malformed_daylight_do_not_fabricate_sun_times() {
        let polar = serde_json::json!({"time":[172800],"sunrise":[null],"sunset":[null],"daylight_duration":[0]});
        assert_eq!(daylight_values(&polar, &serde_json::json!(0), 180000), (None, None, Some(0)));
        let reversed = serde_json::json!({"time":[172800],"sunrise":[220000],"sunset":[200000],"daylight_duration":[90000]});
        assert_eq!(daylight_values(&reversed, &serde_json::json!(0), 180000), (None, None, None));
        assert_eq!(daylight_values(&polar, &serde_json::json!(i64::MIN), 180000), (None, None, None));
    }

    #[test]
    fn malformed_forecast_does_not_discard_current_conditions_and_old_cache_loads() {
        let snapshot = provider_snapshot(r#"{"current":{"temperature_2m":7},"hourly":false,"daily":"bad","timezone":{},"utc_offset_seconds":[]}"#).unwrap();
        assert_eq!(snapshot.temp_c, Some(7));
        assert!(snapshot.precipitation_forecast.is_empty());
        assert_eq!(snapshot.sunrise_at, None);
        assert_eq!(snapshot.timezone, None);
        let mut old = serde_json::to_value(&snapshot).unwrap();
        for key in ["precipitationForecast", "sunriseAt", "sunsetAt", "daylightSeconds", "timezone"] { old.as_object_mut().unwrap().remove(key); }
        assert_eq!(serde_json::from_value::<WeatherSnapshot>(old).unwrap(), snapshot);
    }

    #[test]
    fn permission_clear_removes_forecast_and_daylight() {
        let mut snapshot = snapshot_at(now_unix());
        snapshot.precipitation_forecast.push(PrecipitationHour {at:7200, probability:Some(0), amount_mm:Some(0.0)});
        snapshot.sunrise_at = Some(183600);
        snapshot.sunset_at = Some(226800);
        snapshot.daylight_seconds = Some(43200);
        snapshot.timezone = Some("Europe/Moscow".into());
        snapshot.clear_weather("denied");
        assert!(snapshot.precipitation_forecast.is_empty());
        assert_eq!(snapshot.sunrise_at, None);
        assert_eq!(snapshot.sunset_at, None);
        assert_eq!(snapshot.daylight_seconds, None);
        assert_eq!(snapshot.timezone, None);
    }

    #[test]
    fn omitted_provider_fields_remain_unknown() {
        let snapshot = provider_snapshot(r#"{"current":{"temperature_2m":7}}"#).unwrap();
        assert_eq!(snapshot.temp_c, Some(7));
        assert_eq!(snapshot.feels_c, None);
        assert_eq!(snapshot.wind_kmh, None);
        assert_eq!(snapshot.code, None);
        assert_eq!(snapshot.is_day, None);
        let serialized = serde_json::to_value(snapshot).unwrap();
        assert!(serialized["code"].is_null());
        assert!(serialized["windKmh"].is_null());
    }

    #[test]
    fn invalid_provider_fields_do_not_become_calm_clear_or_daytime() {
        for code in [-1.0, 1.5, 4.0, 100.0] {
            let raw = format!(
                r#"{{"current":{{"temperature_2m":7,"apparent_temperature":1e100,"wind_speed_10m":-1,"weather_code":{code},"is_day":2}}}}"#
            );
            let snapshot = provider_snapshot(&raw).unwrap();
            assert_eq!(snapshot.feels_c, None);
            assert_eq!(snapshot.wind_kmh, None);
            assert_eq!(snapshot.code, None);
            assert_eq!(snapshot.is_day, None);
        }
        assert!(provider_snapshot(r#"{"current":{"temperature_2m":1e100}}"#).is_err());
    }

    #[test]
    fn valid_zero_values_are_distinct_from_unknown() {
        let snapshot = provider_snapshot(r#"{"current":{"temperature_2m":0,"apparent_temperature":0,"wind_speed_10m":0,"weather_code":0,"is_day":0}}"#).unwrap();
        assert_eq!(snapshot.feels_c, Some(0));
        assert_eq!(snapshot.wind_kmh, Some(0));
        assert_eq!(snapshot.code, Some(0));
        assert_eq!(snapshot.is_day, Some(false));
    }

    #[test]
    fn malformed_optional_field_types_do_not_discard_a_valid_temperature() {
        let snapshot = provider_snapshot(r#"{"current":{"temperature_2m":7,"apparent_temperature":"unknown","wind_speed_10m":{},"weather_code":false,"is_day":[]}}"#).unwrap();
        assert_eq!(snapshot.temp_c, Some(7));
        assert_eq!(snapshot.feels_c, None);
        assert_eq!(snapshot.wind_kmh, None);
        assert_eq!(snapshot.code, None);
        assert_eq!(snapshot.is_day, None);
    }

    #[test]
    fn temporary_consent_failure_preserves_observation_and_persisted_access() {
        let state = state_with(Some(snapshot_at(now_unix())));
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        store.kv_set(KEY_ACCESS, b"allowed").unwrap();
        store.kv_set(KEY_SNAPSHOT, b"cached observation").unwrap();
        *state.store.lock().unwrap() = Some(store.clone());
        state.location_allowed.store(true, Ordering::Release);
        let runtime = WeatherRuntime::new();
        runtime.publish(state.clone());
        runtime.set_location_access("requesting").unwrap();
        assert_eq!(runtime.snapshot().unwrap().temp_c, Some(7));
        assert!(!state.location_allowed.load(Ordering::Acquire));
        assert!(!state.commit_snapshot(0, snapshot_at(now_unix())));
        runtime.set_location_access("unavailable").unwrap();
        assert!(state.location_allowed.load(Ordering::Acquire));
        assert!(state.wake.load(Ordering::Acquire));
        let snapshot = runtime.snapshot().unwrap();
        assert_eq!(snapshot.temp_c, Some(7));
        assert_eq!(snapshot.location_status, "retrying");
        assert!(!snapshot.stale);
        assert_eq!(
            store.kv_get(KEY_ACCESS).unwrap().as_deref(),
            Some(b"allowed".as_slice())
        );
        assert!(store.kv_get(KEY_SNAPSHOT).unwrap().is_some());
    }

    #[test]
    fn temporary_device_failure_preserves_freshness_until_observation_expires() {
        let state = state_with(Some(snapshot_at(now_unix())));
        state.location_allowed.store(true, Ordering::Release);
        state.record_location_failure(0, &LocationError::Unavailable("timeout".into()));
        let runtime = WeatherRuntime::new();
        runtime.publish(state.clone());
        let snapshot = runtime.snapshot().unwrap();
        assert_eq!(snapshot.temp_c, Some(7));
        assert_eq!(snapshot.location_status, "retrying");
        assert!(!snapshot.stale);
        state.snapshot.lock().unwrap().as_mut().unwrap().observed_at =
            now_unix().saturating_sub(FRESH_FOR + 1);
        assert!(runtime.snapshot().unwrap().stale);
    }

    #[test]
    fn denied_device_failure_revokes_and_prevents_old_generation_publication() {
        let state = state_with(Some(snapshot_at(now_unix())));
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        store.kv_set(KEY_ACCESS, b"allowed").unwrap();
        store.kv_set(KEY_SNAPSHOT, b"cached observation").unwrap();
        *state.store.lock().unwrap() = Some(store.clone());
        state.location_allowed.store(true, Ordering::Release);
        state.record_location_failure(0, &LocationError::Denied("access denied".into()));
        assert!(!state.location_allowed.load(Ordering::Acquire));
        assert!(!state.commit_snapshot(0, snapshot_at(now_unix())));
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.temp_c, None);
        assert_eq!(snapshot.location_status, "denied");
        assert_eq!(
            store.kv_get(KEY_ACCESS).unwrap().as_deref(),
            Some(b"denied".as_slice())
        );
        assert!(store.kv_get(KEY_SNAPSHOT).unwrap().is_none());
    }

    #[test]
    fn startup_restore_cannot_override_an_early_consent_result() {
        let state = state_with(Some(WeatherSnapshot::empty("permission-required")));
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        *state.store.lock().unwrap() = Some(store);
        let runtime = WeatherRuntime::new();
        runtime.publish(state.clone());
        runtime.set_location_access("requesting").unwrap();
        state.restore();
        assert_eq!(runtime.snapshot().unwrap().location_status, "requesting");
        assert!(!state.location_allowed.load(Ordering::Acquire));
    }

    #[test]
    fn provider_errors_never_carry_device_coordinates() {
        let url = "https://api.open-meteo.com/v1/forecast?latitude=59.96749444009462&longitude=30.271205522843143&current=x";
        let message = redact_query(&format!(
            "open-meteo request failed: {url}: Network Error: timed out"
        ));
        assert!(!message.contains("59.967"), "{message}");
        assert!(!message.contains("30.271"), "{message}");
        assert!(message.contains("timed out"), "{message}");
    }

    #[test]
    fn shutdown_does_not_wait_for_a_cache_publication_lock() {
        let state = state_with(None);
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let locker = thread::spawn({
            let state = state.clone();
            move || {
                let _guard = state.publication.lock().unwrap();
                locked_tx.send(()).unwrap();
                let _ = release_rx.recv_timeout(Duration::from_secs(1));
            }
        });
        locked_rx.recv().unwrap();
        let started = Instant::now();
        state.stop();
        let elapsed = started.elapsed();
        let _ = release_tx.send(());
        locker.join().unwrap();
        assert!(elapsed < Duration::from_millis(500));
    }

    #[test]
    fn device_fix_with_failed_weather_ends_locating_without_fabricated_temperature() {
        let state = state_with(Some(WeatherSnapshot::empty("locating")));
        state.location_allowed.store(true, Ordering::Release);
        state.record_weather_failure(0);
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert_eq!(snapshot.location_status, "available");
        assert_eq!(snapshot.location_source.as_deref(), Some("windows"));
        assert_eq!(snapshot.temp_c, None);
        assert!(!snapshot.weather_available);
    }

    #[test]
    fn weather_accepts_known_windows_sources_and_rejects_unknown_provenance() {
        for source in [0, 1, 2, 3, 5, 6] {
            assert!(accepts_position_source(source));
        }
        for source in [4, -1, 99] {
            assert!(!accepts_position_source(source));
        }
        assert_eq!(position_method(3), Some("ip"));
        assert_eq!(position_method(5), Some("default"));
        assert_eq!(position_method(6), Some("coarse"));
    }

    #[test]
    fn windows_network_estimate_provenance_reaches_public_snapshot() {
        let fix = Location { method: Some("ip".into()), accuracy_m: Some(1500.0), ..device_fix() };
        let parsed = serde_json::from_value(serde_json::json!({"current":{"temperature_2m":11,"time":100}})).unwrap();
        let snapshot = snapshot_from_response(parsed, &fix, 100).unwrap();
        let public = serde_json::to_value(snapshot).unwrap();
        assert_eq!(public["locationMethod"], "ip");
        assert_eq!(public["accuracyM"], 1500.0);
        assert!(public.get("latitude").is_none());
        assert!(public.get("longitude").is_none());
    }

    #[test]
    fn source_change_invalidates_city_cache_and_previous_weather() {
        let state = state_with(Some(snapshot_at(now_unix())));
        state.location_allowed.store(true, Ordering::Release);
        let fix = Location { method: Some("ip".into()), ..device_fix() };
        assert_eq!(state.prepare_place_name(0, &fix), Some(true));
        assert!(state.commit_place_name(0, &fix, Ok("Network city".into())));
        *state.observation_fix.lock().unwrap() = Some(fix);
        assert_eq!(state.prepare_place_name(0, &device_fix()), Some(true));
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert!(snapshot.location.is_empty());
        assert!(!snapshot.weather_available);
        assert_eq!(snapshot.location_method.as_deref(), Some("wifi"));
    }

    #[test]
    fn manually_set_windows_location_never_calls_client_reverse_geocoder() {
        let fix = Location { method: Some("default".into()), ..device_fix() };
        assert_eq!(lookup_place_name(&fix).unwrap(), ("Место задано в Windows".into(), PlaceProvider::Windows));
    }

    #[test]
    fn old_snapshot_without_position_metadata_deserializes() {
        let mut public = serde_json::to_value(snapshot_at(100)).unwrap();
        public.as_object_mut().unwrap().remove("locationMethod");
        public.as_object_mut().unwrap().remove("accuracyM");
        let snapshot: WeatherSnapshot = serde_json::from_value(public).unwrap();
        assert_eq!(snapshot.location_method, None);
        assert_eq!(snapshot.accuracy_m, None);
    }

    #[test]
    fn legacy_ip_weather_cache_is_removed_and_ip_indicator_survives_without_weather() {
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        store
            .kv_set(
                "weather/snapshot",
                &serde_json::to_vec(&snapshot_at(now_unix())).unwrap(),
            )
            .unwrap();
        store
            .kv_set("weather/location", br#"{"latitude":55,"longitude":37}"#)
            .unwrap();
        let ip = IpLocation {
            city: "VPN city".into(),
            country: "VPN country".into(),
            ip: "203.0.113.4".into(),
            resolved_at: now_unix(),
            stale: false,
        };
        store
            .kv_set(KEY_IP, &serde_json::to_vec(&ip).unwrap())
            .unwrap();
        let state = state_with(None);
        *state.store.lock().unwrap() = Some(store.clone());
        state.restore();
        let runtime = WeatherRuntime::new();
        runtime.publish(state);
        let snapshot = runtime.snapshot().unwrap();
        assert_eq!(snapshot.temp_c, None);
        assert!(!snapshot.weather_available);
        assert_eq!(snapshot.location_source, None);
        assert_eq!(snapshot.location_status, "permission-required");
        assert_eq!(snapshot.ip_location, Some(ip));
        assert!(store.kv_get("weather/snapshot").unwrap().is_none());
        assert!(store.kv_get("weather/location").unwrap().is_none());
    }

    #[test]
    fn permission_revocation_invalidates_requests_and_clears_only_weather() {
        let state = state_with(Some(snapshot_at(now_unix())));
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        *state.store.lock().unwrap() = Some(store.clone());
        state.snapshot.lock().unwrap().as_mut().unwrap().ip_location = Some(IpLocation {
            city: "Network".into(),
            country: String::new(),
            ip: "203.0.113.5".into(),
            resolved_at: now_unix(),
            stale: false,
        });
        let runtime = WeatherRuntime::new();
        runtime.publish(state.clone());
        runtime.set_location_access("allowed").unwrap();
        let generation = state.access_revision.load(Ordering::Acquire);
        runtime.set_location_access("denied").unwrap();
        assert!(!state.location_allowed.load(Ordering::Acquire));
        assert_ne!(state.access_revision.load(Ordering::Acquire), generation);
        let snapshot = runtime.snapshot().unwrap();
        assert_eq!(snapshot.temp_c, None);
        assert_eq!(snapshot.location_status, "denied");
        assert!(snapshot.ip_location.is_some());
        assert!(
            !state.commit_snapshot(generation, snapshot_at(now_unix())),
            "an in-flight provider response must not restore weather after revocation"
        );
        assert!(store.kv_get(KEY_SNAPSHOT).unwrap().is_none());
        assert_eq!(runtime.snapshot().unwrap().temp_c, None);
    }

    #[test]
    fn consent_before_deferred_store_open_persists_the_latest_decision() {
        for status in ["allowed", "denied"] {
            let state = state_with(Some(WeatherSnapshot::empty("permission-required")));
            let runtime = WeatherRuntime::new(); runtime.publish(state.clone());
            runtime.set_location_access(status).unwrap();
            let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
            store.kv_set(KEY_ACCESS, if status == "allowed" { b"denied" } else { b"allowed" }).unwrap();
            state.attach_store(store.clone()); state.restore();
            assert_eq!(store.kv_get(KEY_ACCESS).unwrap().unwrap(), status.as_bytes());
            assert_eq!(state.location_allowed.load(Ordering::Acquire), status == "allowed");
        }
    }

    #[test]
    fn timed_out_early_consent_recovers_persisted_access_after_deferred_open() {
        let state = state_with(Some(WeatherSnapshot::empty("permission-required")));
        let runtime = WeatherRuntime::new(); runtime.publish(state.clone());
        runtime.set_location_access("requesting").unwrap();
        runtime.set_location_access("unavailable").unwrap();
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        store.kv_set(KEY_ACCESS, b"allowed").unwrap();
        state.attach_store(store.clone());
        assert!(state.location_allowed.load(Ordering::Acquire));
        assert_eq!(store.kv_get(KEY_ACCESS).unwrap().unwrap(), b"allowed");
    }

    #[test]
    fn requesting_is_transient_and_does_not_grant_or_persist_access() {
        let state = state_with(Some(WeatherSnapshot::empty("permission-required")));
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        *state.store.lock().unwrap() = Some(store.clone());
        let runtime = WeatherRuntime::new();
        runtime.publish(state.clone());
        runtime.set_location_access("requesting").unwrap();
        assert!(!state.location_allowed.load(Ordering::Acquire));
        assert_eq!(state.access_revision.load(Ordering::Acquire), 1);
        assert!(store.kv_get(KEY_ACCESS).unwrap().is_none());
        assert_eq!(runtime.snapshot().unwrap().location_status, "requesting");
        assert!(!state.commit_snapshot(0, snapshot_at(now_unix())));
    }

    #[test]
    fn restored_weather_snapshot_provides_stale_cache_immediately_after_restart() {
        let state = state_with(None);
        let store: Arc<dyn unidtk_api::ModuleDb> = Arc::new(unidtk_api::InMemoryModuleDb::new());
        store.kv_set(KEY_ACCESS, b"allowed").unwrap();
        store
            .kv_set(
                KEY_SNAPSHOT,
                &serde_json::to_vec(&snapshot_at(now_unix())).unwrap(),
            )
            .unwrap();
        *state.store.lock().unwrap() = Some(store);
        state.restore();
        let snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        assert!(state.location_allowed.load(Ordering::Acquire));
        assert_eq!(snapshot.temp_c, Some(7));
        assert!(snapshot.weather_available);
        assert!(snapshot.stale);
        assert_eq!(snapshot.location, "Testville");
    }

    #[test]
    fn periodic_wait_reaches_deadline_instead_of_spinning_forever_at_zero() {
        let state = state_with(None);
        let started = Instant::now();
        state.wait_delay(Duration::from_millis(15));
        assert!(started.elapsed() >= Duration::from_millis(15));
        assert!(started.elapsed() < Duration::from_secs(1));
        state.wake.store(true, Ordering::Release);
        let started = Instant::now();
        state.wait_delay(Duration::from_secs(3600));
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    fn snapshot_at(observed_at: u64) -> WeatherSnapshot {
        WeatherSnapshot {
            temp_c: Some(7),
            feels_c: Some(5),
            wind_kmh: Some(12),
            code: Some(71),
            is_day: Some(true),
            location: "Testville".into(),
            location_name_status: "available".into(),
            location_name_provider: Some("bigdatacloud".into()),
            observed_at,
            stale: false,
            weather_available: true,
            location_source: Some("windows".into()),
            location_method: Some("wifi".into()),
            accuracy_m: Some(100.0),
            location_status: "available".into(),
            ip_location: None,
            precipitation_forecast: Vec::new(),
            sunrise_at: None,
            sunset_at: None,
            daylight_seconds: None,
            timezone: None,
        }
    }

    fn state_with(snapshot: Option<WeatherSnapshot>) -> Arc<WeatherState> {
        Arc::new(WeatherState {
            store: Mutex::new(None),
            snapshot: Mutex::new(snapshot),
            place_name: Mutex::new(None),
            observation_fix: Mutex::new(None),
            failures: AtomicU32::new(0),
            location_allowed: AtomicBool::new(false),
            access_revision: AtomicU32::new(0),
            wake: AtomicBool::new(false),
            network_revision: AtomicU32::new(0),
            ip_revision: AtomicU32::new(0),
            weather_revision: AtomicU32::new(0),
            name_revision: AtomicU32::new(0),
            vpn_active: AtomicBool::new(false),
            access_decision: Mutex::new(None),
            publication: Mutex::new(()),
            stop: Arc::new(AtomicBool::new(false)),
            thread: Mutex::new(None),
        })
    }

    #[test]
    fn absurd_coordinates_are_rejected() {
        // Null island is what a provider answering "null" for every field hands
        // back, and it is a real place — so it must not pass.
        let null_island = Location {
            latitude: 0.0,
            longitude: 0.0,
            name: String::new(),
            method: None,
            accuracy_m: None,
            resolved_at: 0,
        };
        assert!(!null_island.is_plausible());

        let nan = Location {
            latitude: f64::NAN,
            longitude: 10.0,
            name: String::new(),
            method: None,
            accuracy_m: None,
            resolved_at: 0,
        };
        assert!(!nan.is_plausible());

        let out_of_range = Location {
            latitude: 91.0,
            longitude: 10.0,
            name: String::new(),
            method: None,
            accuracy_m: None,
            resolved_at: 0,
        };
        assert!(!out_of_range.is_plausible());
    }

    #[test]
    fn a_real_device_coordinate_passes() {
        let fresh = Location {
            latitude: 55.7558,
            longitude: 37.6173,
            name: "Moscow".into(),
            method: Some("wifi".into()),
            accuracy_m: Some(100.0),
            resolved_at: now_unix(),
        };
        assert!(fresh.is_plausible());
    }

    #[test]
    fn a_runtime_clone_sees_what_init_published() {
        // The regression this guards: a module whose handlers read a *different*
        // instance than the one init filled reports "no weather" forever.
        let runtime = WeatherRuntime::new();
        let clone = runtime.clone();
        assert!(clone.snapshot().is_none());

        runtime.publish(state_with(Some(snapshot_at(now_unix()))));
        assert_eq!(clone.snapshot().and_then(|s| s.temp_c), Some(7));
    }

    #[test]
    fn an_empty_cache_reports_no_observation_rather_than_a_zero_one() {
        let runtime = WeatherRuntime::new();
        runtime.publish(state_with(None));
        assert!(runtime.snapshot().is_none());
    }

    #[test]
    fn an_aged_snapshot_is_reported_stale_at_read_time() {
        let runtime = WeatherRuntime::new();
        runtime.publish(state_with(Some(snapshot_at(
            now_unix().saturating_sub(FRESH_FOR + 60),
        ))));
        assert!(runtime.snapshot().expect("snapshot").stale);
    }

    #[test]
    fn a_fresh_snapshot_is_not_stale() {
        let runtime = WeatherRuntime::new();
        runtime.publish(state_with(Some(snapshot_at(now_unix()))));
        assert!(!runtime.snapshot().expect("snapshot").stale);
    }

    #[test]
    fn reading_without_a_store_yields_no_location_instead_of_a_panic() {
        assert!(state_with(None)
            .snapshot
            .lock()
            .expect("snapshot")
            .is_none());
    }

    #[test]
    fn consecutive_failures_stretch_the_wait_and_success_resets_it() {
        let state = state_with(None);
        assert_eq!(state.next_delay(), REFRESH);

        state.failures.store(1, Ordering::Relaxed);
        let second = state.next_delay();
        assert_eq!(second, Duration::from_secs(5));

        state.failures.store(99, Ordering::Relaxed);
        assert_eq!(
            state.next_delay(),
            BACKOFF_MAX,
            "the backoff must be capped, not unbounded"
        );

        state.failures.store(0, Ordering::Relaxed);
        assert_eq!(state.next_delay(), REFRESH);
    }

    #[test]
    fn unavailable_weather_does_not_delay_ip_or_name_retry() {
        let mut ip = RefreshSchedule::new(Duration::from_secs(LOCATION_FOR));
        let mut weather = RefreshSchedule::new(REFRESH);
        let mut name = RefreshSchedule::new(Duration::from_secs(NAME_FOR));
        ip.finish(true); weather.finish(false); name.finish(false);
        assert_eq!(ip.failures, 0);
        assert_eq!(weather.failures, 1);
        assert_eq!(name.failures, 1);
        assert!(weather.due < ip.due);
        assert!(name.due < ip.due);
        for _ in 0..100 { weather.finish(false); }
        assert!(weather.due.saturating_duration_since(Instant::now()) <= BACKOFF_MAX);
        assert_eq!(name.failures, 1);
        weather.finish(true);
        assert_eq!(weather.failures, 0);
    }

    #[test]
    fn changed_route_rejects_weather_and_name_results_and_marks_old_payloads_stale() {
        let state = state_with(Some(snapshot_at(now_unix())));
        state.location_allowed.store(true, Ordering::Release);
        let old = state.network_revision.load(Ordering::Acquire);
        state.network_changed();
        let fix = device_fix();
        assert!(!state.commit_snapshot_on_network(0, old, snapshot_at(now_unix()), Some(&fix)));
        assert!(!state.commit_place_name_on_network(0, old, &fix, Ok(("Old place".into(), PlaceProvider::BigDataCloud))));
        let mut snapshot = state.snapshot.lock().unwrap().clone().unwrap();
        snapshot.weather_available = true;
        snapshot.location_name_status = "available".into();
        snapshot.location = "Old place".into();
        state.mark_network_freshness(&mut snapshot);
        assert!(snapshot.stale);
        assert_eq!(snapshot.location, "Old place");
    }

    #[test]
    fn response_from_previous_route_cannot_replace_the_current_ip() {
        let state = state_with(Some(WeatherSnapshot::empty("permission-required")));
        let request_generation = state.network_revision.load(Ordering::Acquire);
        state.network_changed();
        let old_ip = IpLocation { city: "Old exit".into(), country: "".into(),
            ip: "192.0.2.1".into(), resolved_at: now_unix(), stale: false };
        assert!(!state.commit_ip(request_generation, old_ip.clone()));
        assert!(state.snapshot.lock().unwrap().as_ref().unwrap().ip_location.is_none());
        assert!(state.commit_ip(state.network_revision.load(Ordering::Acquire), old_ip));
        assert!(state.snapshot.lock().unwrap().as_ref().unwrap().ip_location.is_some());
    }

    #[test]
    fn route_change_invalidates_an_in_flight_network_generation_and_wakes_fresh_cache() {
        let state = state_with(None);
        let before = state.network_revision.load(Ordering::Acquire);
        let mut ip = RefreshSchedule::new(Duration::from_secs(LOCATION_FOR));
        ip.finish(true);
        assert!(ip.due > Instant::now());
        state.network_changed();
        assert_ne!(before, state.network_revision.load(Ordering::Acquire));
        assert!(state.wake.load(Ordering::Acquire));
        ip.wake();
        assert!(ip.due <= Instant::now());
        state.stop.store(true, Ordering::Release);
        let stopped = state.network_revision.load(Ordering::Acquire);
        state.network_changed();
        assert_eq!(stopped, state.network_revision.load(Ordering::Acquire));
    }

    #[test]
    fn stopping_returns_immediately_even_while_the_worker_is_busy() {
        // The regression this guards: `stop` used to join the worker, and the
        // worker can be inside a network call that `ureq` cannot cancel. That
        // turned a 1.5s shutdown budget into a 4s freeze of the whole desktop.
        //
        // The slow worker is installed as the state's *own* thread, so `stop` has
        // to actually deal with it — a bystander thread would prove nothing.
        let state = state_with(None);
        let gate = Arc::new(AtomicBool::new(false));
        let busy = thread::spawn({
            let gate = Arc::clone(&gate);
            move || {
                // Stands in for an in-flight HTTP request: released only once the
                // test has confirmed `stop` already returned.
                while !gate.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        });
        *state.thread.lock().expect("thread slot") = Some(busy);

        let started = std::time::Instant::now();
        state.stop();
        let elapsed = started.elapsed();

        gate.store(true, Ordering::Relaxed);
        assert!(
            state.thread.lock().expect("thread slot").is_none(),
            "stop must take the handle so nothing else joins it twice"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "stop must not wait for the worker, took {elapsed:?}"
        );
    }

    #[test]
    fn the_snapshot_serialises_with_the_shape_the_panel_expects() {
        let raw = serde_json::to_value(snapshot_at(1_700_000_000)).expect("serialize");
        assert_eq!(raw["tempC"], 7);
        assert_eq!(raw["feelsC"], 5);
        assert_eq!(raw["windKmh"], 12);
        assert_eq!(raw["code"], 71);
        assert_eq!(raw["isDay"], true);
        assert_eq!(raw["observedAt"], 1_700_000_000u64);
        assert_eq!(raw["stale"], false);
    }

    #[test]
    fn a_cached_observation_survives_a_round_trip_through_the_store_format() {
        let raw = serde_json::to_vec(&snapshot_at(1_700_000_000)).expect("encode");
        let back: WeatherSnapshot = serde_json::from_slice(&raw).expect("decode");
        assert_eq!(back.temp_c, Some(7));
        assert_eq!(back.location, "Testville");
    }
}
