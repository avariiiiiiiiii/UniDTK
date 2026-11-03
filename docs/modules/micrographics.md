# The Micrographics Module: From a Windows Fix to a Panel of Symbols

This chapter explains the `micrographics` module step by step. The module puts the date, the time, the current weather, the place name, a public-network indicator, and the offline state on the desktop as small typographic cells. It shows where the coordinates come from and why they come only from Windows positioning after an explicit user action, how the weather request is made and how often, what is kept on disk, how the cells are laid out around the other widgets, and what the panel shows when a source fails.

Every mechanism here is taken from the code. Line numbers refer to the files as they are in this repository.

## What you will understand by the end

- Why the door handlers only read an in-memory snapshot, and which thread makes the network calls.
- How a Windows position is requested, filtered by its provenance, and turned into a place name through a provider chain.
- How the weather, the public-IP indicator, and the place name run on three clocks with separate backoff.
- What is stored, under which keys, and what is restored after a restart.
- How the panel turns a snapshot into cells and packs them around the permanent widgets.

## Files involved

| File | Role |
| :--- | :--- |
| [modules/micrographics/rust/micrographics.rs](../../modules/micrographics/rust/micrographics.rs) | The module: fetching, caching, the refresh thread, and the door handler (3,091 lines). |
| [modules/micrographics/ui/widget.js](../../modules/micrographics/ui/widget.js) | The panel: cell reconciliation, layout calls, the edit-mode controls, and the event subscriptions. |
| [modules/micrographics/ui/composition.js](../../modules/micrographics/ui/composition.js) | Pure functions: `buildTokens` turns a snapshot into cells, `routeElements` places them. |
| [modules/micrographics/ui/metadata.json](../../modules/micrographics/ui/metadata.json) | Module declaration: `backend: true`, `singleton: true`, the tints, and the `mgLayout` look option. |
| [modules/micrographics/ui/location-permission.html](../../modules/micrographics/ui/location-permission.html) | The page of the temporary window that hosts the Windows consent prompt. |
| [core/kernel/weather_location.rs](../../core/kernel/weather_location.rs) | The kernel side of the permission request: the window, the foreground check, and the timeout. |
| [core/kernel/command_inventory.rs](../../core/kernel/command_inventory.rs) | Declares `weather_current` as read-only and `weather_location_request` as write. |

The crate `unidtk-mod-micrographics` depends on `unidtk-api` and not on the kernel, as [modules.contract-only](../reference/modules.md#r-modules.contract-only) requires.

---

## Step 1: What the panel shows

`buildTokens` in `composition.js` takes the current time and the latest snapshot and returns a flat list of tokens. A token has an `id`, a `group`, a kind (`val`, `lbl`, `ic`, `outline`, `off`, `warning`, and a few others), a value, and a `title` that holds the source line for screen readers. Nothing is drawn yet at this point.

The groups, in the order `buildTokens` adds them, are `date`, `time`, `freshness`, `temperature`, `condition`, `feels`, `wind`, `precipitation`, `precipitation-amount`, `sunrise`, `sunset`, `daylight`, `geolocation`, `location`, `ip-location`, `network`, and `volume`. Date and time are always present, and the system volume is added when a volume value exists. The other groups are added only when their data exists. Weather cells appear only when `weatherAvailable` is true and `tempC` is a number.

Filename: modules/micrographics/ui/composition.js (abridged)

```js
add("weather:temperature", "temperature", f.stale ? "off" : "val", `${signed(weather.tempC)}°C`, source);
```

A stale value keeps its cell but changes its kind to `off`, so it is never presented as current. The source line in `title` names the positioning method and the age, so the detail needs no popup.

Unknown values are shown as unknown, not as zero. A missing wind speed becomes `НЕТ ДАННЫХ`. The weather code is translated by a table in the same file, and a code the table does not know gets the label `ПОГОДА НЕИЗВЕСТНА`. The reason is the same in both cases: a fabricated `0 км/ч` would look like a real reading.

---

## Step 2: One thread fetches, the door only reads

The Rust module follows one rule, stated at the top of `micrographics.rs`: the only place a network request happens is the background refresh thread that `init` starts. The door handler, the command `weather_current`, only reads memory.

Filename: modules/micrographics/rust/micrographics.rs (abridged)

```rust
fn init(&self, ctx: unidtk_api::ModuleContext) -> Result<(), String> {
    // ... diagnostic and disabled cases return early ...
    self.runtime.publish(Arc::clone(&state));
    // subscribes to "network-changed", "system-resumed", and "vpn"
    let handle = thread::Builder::new().name("weather".to_string()).spawn(move || {
        // open the cache store "weather", then run the refresh loop
    })?;
    // ...
}
```

`WeatherRuntime` is a small clonable handle around one shared cell. The kernel registers a clone as managed state, and `init` fills the same cell. `snapshot()` returns a copy of the latest snapshot, with staleness decided at read time, so a cached value cannot claim to be fresh hours after it was taken. Without a store or before the first observation, the door says so: `weather: no observation yet`. It does not return a zeroed reading.

The socket door and the UI door both reach this function. In the kernel, `weather_current` is a read-only command in [core/kernel/command_inventory.rs](../../core/kernel/command_inventory.rs) (line 159), and its UI handler is in [core/kernel/main.rs](../../core/kernel/main.rs) (line 403).

The panel gets its data in two ways, both in `widget.js`. It listens for the `weather-update` event, which the worker emits after every committed change. On load it also calls `weather_current` through `invoke`, retrying at 0, 250, 1000, and 3000 ms because the managed state may not exist yet. A one-second timer calls `schedule()`, but only while the document is visible, and a repaint runs layout only when the cell signature changes.

---

## Step 3: Asking Windows for permission

Permission is never requested by the worker. The only way to start the request is the "Определить место" button, which appears in edit mode when the location status is neither `available` nor `retrying`. The button calls `weather_location_request`, a write command, and shows "Определяю…" until the call returns.

The kernel side is in [core/kernel/weather_location.rs](../../core/kernel/weather_location.rs):

1. An atomic flag `IN_FLIGHT` rejects a second request. The state becomes `requesting`, which bumps the access revision and makes older results stale.
2. A worker thread starts. If the application is not the foreground process, it shows a small window (label `weather-location-permission`, 440 × 250, page `location-permission.html`) and waits for it to take focus. A second thread completes the request as `unavailable` after 15 seconds if no consent dialog has started.
3. `Geolocator::RequestAccessAsync` runs on the UI thread. Its answer is awaited on a separate thread, polled for up to 60 seconds, and mapped to `allowed`, `denied`, or `unavailable`.
4. `complete` records the answer, closes the window, and clears `IN_FLIGHT`.

Warning: the code says `WebviewWindowBuilder` can deadlock inside event callbacks on Windows. That is why the window is built on the request worker and not on the UI thread. The UI thread only starts the WinRT call and never waits for its answer, which keeps the request within [kernel.no-blocking-main-thread](../reference/kernel.md#r-kernel.no-blocking-main-thread).

A closed window, a timeout, or a failed WinRT call completes as `unavailable`. That is not a statement about permission, so the persisted decision is restored. Only an explicit `denied` clears the weather.

---

## Step 4: Reading the Windows position

The refresh thread reads the position with `device_location`, and only when `location_allowed` is true. The read uses the WinRT `Geolocator` with high accuracy, in two phases:

1. A fast phase accepts a cached fix up to 15 minutes old, with a 2-second timeout. It polls for up to 2 seconds.
2. If the fast phase does not complete, the request is cancelled and repeated with no age limit and a 20-second timeout (`GEO_TIMEOUT`).

After the result arrives, the code checks its provenance. `position_method` maps the WinRT `PositionSource` code to a method name, and any other code is rejected.

The WinRT `PositionSource` code maps through `position_method()`: `0` cellular, `1` satellite, `2` wifi, `3` ip, `5` default, `6` coarse. Any other code is rejected as unknown provenance.

Two more checks follow. A position whose source is `ip` (3) is discarded while the VPN is active, because the network estimate then describes the VPN gateway and not the user. And coordinates must be finite, within ±90 and ±180, and not both zero.

The accuracy comes from `Coordinate.Accuracy` and is kept as `accuracy_m`. Method `default` means Windows was given a place by hand. For that case the module names the place "Место задано в Windows" and asks no geocoder.

---

## Step 5: From coordinates to a place name

`lookup_place_name` is a chain with a fixed order: BigDataCloud first, then OpenStreetMap Nominatim. Each provider can be on a cooldown, and a provider that is cooling down is skipped without a request. A `default` fix never reaches the chain.

| Provider | Request | Accepted only if | Cooldown on failure |
| :--- | :--- | :--- | :--- |
| BigDataCloud | `reverse-geocode-client`, with the coordinates and `localityLanguage=ru` | the answer says `lookupSource` is `coordinates` or `reverseGeocoding`, and its echoed coordinates match the fix within 0.00001° | 10 min if unreachable, 60 min if refused (403 or 429), none if the answer is unusable |
| OpenStreetMap | `nominatim.openstreetmap.org/reverse`, `zoom=12`, `accept-language=ru` | the queried point lies inside the returned `boundingbox` | the same table |

The name is the first non-empty value among the fields below, with no control characters and at most 160 characters. For BigDataCloud the order is `city`, `locality`, `principalSubdivision`. For Nominatim it is `city`, `town`, `village`, `municipality`, `city_district`, `county`, `state`.

Warning: the code comment says the bounding box is what ties the name to the place. Nominatim answers with the centroid of the matched feature, so the coordinates in the answer can sit far from the query, and an echo check would not work for it.

The name is kept only while the device is still in the same place. A place match counts the uncertainty Windows published: the radius is the claimed accuracy when it is above 1,000 m, and 1,000 m otherwise. A cached name is used only if its method is the same and it is younger than 30 minutes. If the place has moved, the old weather is cleared and the name is looked up again. A failed lookup keeps the last good name on screen.

Attribution is part of the output. The panel adds a source line that names the provider that answered (`bigdatacloud` or `openstreetmap`), and the OpenStreetMap line carries the required credit. The provider is recorded with the name, so a name from one provider is never labelled with the other's name.

---

## Step 6: The public-network indicator

The IP indicator answers a different question: which network the user exits to. The module's header says it is never a fallback for the weather.

`lookup_ip_location` asks `ipapi.co/json/` first and `ipwho.is` as the fallback. A `403` or `429` sets a cooldown from `Retry-After`, clamped to 60 seconds through 24 hours. The indicator is refreshed every 30 minutes and after a route change, and it does not need the permission. It stores `{ city, country, ip, resolvedAt, stale }` and no coordinates.

---

## Step 7: The weather request

`fetch_snapshot` makes one request to Open-Meteo with the device coordinates. The URL asks for the current values, two hourly series, the daily sunrise, sunset, and day length, and a two-day forecast in the place's own time zone.

The URL is `api.open-meteo.com/v1/forecast` with the device latitude and longitude, `current=temperature_2m,apparent_temperature,wind_speed_10m,is_day,weather_code`, hourly precipitation probability and amount, daily sunrise, sunset, and `daylight_duration`, `timeformat=unixtime`, `timezone=auto`, and `forecast_days=2`.

Each value is validated on its own. A field that is missing, not finite, or out of range becomes `None`. Only a missing `current` block or a missing temperature fails the whole snapshot. The weather code must be one of the WMO codes the module lists, and `is_day` must be `0` or `1`. The sun times must belong to the local day that `utc_offset_seconds` defines, and a polar day or night leaves them unset.

The precipitation forecast takes three hourly windows that start within the next hour, plus one reserve hour. The reserve is optional, and it keeps three windows valid across an hour boundary between refreshes. The windows are rejected if they have gaps.

On success, `commit_snapshot_on_network` checks that the access revision and the network revision are unchanged, stores the snapshot as the current value, copies over the IP indicator and the place name, and writes it to the cache. A result that arrives after a route change or a permission change is discarded.

On failure, `record_weather_failure` keeps the old values. It keeps `weather_available` only if a temperature was already known, and it marks the snapshot stale after 20 minutes.

---

## Step 8: Three clocks and a backoff

The refresh thread keeps three independent `RefreshSchedule` values, so one unavailable provider does not delay another.

| Clock | Healthy interval | Runs only when |
| :--- | :--- | :--- |
| IP indicator | 30 min (`LOCATION_FOR`) | always, once the thread runs |
| Weather (with the device fix) | 15 min (`REFRESH`) | location is allowed |
| Place name | 30 min (`NAME_FOR`), or at once when the place moved | location is allowed and a fix exists |

Each failure stretches the next attempt: 5 s after the first failure, then 10, 20, 40, 80, 160 s, capped at 5 minutes (`BACKOFF_MAX`). A success resets the count. A failed place lookup uses the same table.

The thread does not poll. `wait_delay` parks it until the earliest due time or an unpark. Four events wake it: `network-changed` (emitted in [core/kernel/network.rs](../../core/kernel/network.rs)), `system-resumed` (emitted in [core/kernel/shutdown.rs](../../core/kernel/shutdown.rs)), a change of the VPN's `(connected, selected_tag)` pair, and a change of the permission decision. A network or permission change wakes all three clocks and clears the held position, so nothing from the old route is reused.

---

## Step 9: Three levels of "old"

- The backend marks the weather stale after 20 minutes (`FRESH_FOR`), decided at read time in `snapshot()`, and the IP indicator after 30 minutes.
- A route change marks weather, IP, and place name stale until a fetch on the new route replaces them. Each has a revision counter compared with the network revision.
- The panel's `freshness()` shows the `УСТАРЕЛО` warning after 1,200 seconds, and the IP label goes stale after 1,800 seconds. A stale value gets kind `off`, so it stays visible but does not look current.

---

## Step 10: What is stored

The module's cache store is opened with `StoreSpec::cache("weather")`. The keys below are the ones written by the code.

| Key | Written when | Contents |
| :--- | :--- | :--- |
| `windows/snapshot-v2` | every committed weather or place-name result | the full `WeatherSnapshot`: the weather, the place name and its provider, the method and accuracy, and the IP indicator field as it was at the time |
| `network/ip-location-v1` | every successful IP lookup | the IP indicator |
| `windows/access-allowed-v1` | every permission decision except `unavailable` | `allowed` or `denied` |
| `windows/place-fix-v1` | every successful place lookup | the place name, its provider, its method, its time, and the coordinates it was resolved for |

On restart, `restore()` runs first. It removes the old keys `weather/snapshot` and `weather/location`, because they came from IP-derived coordinates, and it does not restore them. If permission was granted, the cached snapshot comes back marked stale, so the panel shows the last values with the warning at once. If permission was not granted, the snapshot starts empty with status `permission-required`, and only the IP indicator is restored.

The panel's own layout settings are stored elsewhere. `settingsFor` keeps `x`, `y`, `mgScale`, and `mgStyle` in `config.positions[id]` and saves them through `saveConfig()` from `core/ui/unidtk-core.js`. The module does not decide where that file lives.

---

## Step 11: How the cells are laid out

The layout runs in `widget.js` on the browser side. It gets three inputs: the parent viewport, the geometry of the permanent cards (`getDockGeometry()`), and the user's anchor and scale. The anchor is the top-left corner the user dragged the block to. The scale changes with the corner handle, between 0.55 and 3.

`routeElements` in `composition.js` places the cells in three steps.

1. `flow()` walks the groups in order and puts each one in the nearest free interval of a row. A group stays together when it fits. Intervals are cut around the docks, so the block wraps around them. It tries the anchor first, then the left margin.
2. If `flow()` cannot place everything, a candidate search tries a small set of positions and widths taken from the anchor, the docks, and the previous layout. It picks the one that fits best.
3. If no candidate fits, the whole composition is kept in a scrollable area, and the grid gets `tabIndex` 0 and the class `mg-overflow`. If the screen has no free area at all, it is drawn as an overlay and is still scrollable. No cell is dropped.

`settingsFor()` in `widget.js` clamps the scale to 0.55 through 3, falls back to the `mechanical` style, and drops the obsolete width and height keys. The three styles (`mechanical`, `outline`, `symbols`) change only the wording and the preferred width of the block, which is 430, 340, or 290 px times the scale. A cell's font is not shrunk to fit, so a crowded screen shows more rows and not smaller type.

The `pills` layout draws one tinted pill behind each run of a group. It is used only when the document has `data-material="jelly"` and `data-mg-layout="pills"`, which the theme engine writes from the theme's `mgLayout` field; the choice in the Look menu is stored in the theme, not in the card.

Cells are reused, not rebuilt. `paintCell` changes only the text, the kind, and the glyph of an existing node, and the layout reads the measured sizes in one batch before it writes positions. A relayout keeps the DOM nodes and the scroll position.

The `capabilities` of the permanent cards suppress groups they already show. If a clock shows the date, the module's own date cells are not built.

---

## Step 12: What the panel shows when a source fails

| Situation | What the panel shows | Where |
| :--- | :--- | :--- |
| Permission never granted | `РАЗРЕШИ ГЕОЛОКАЦИЮ`, no weather cells, and the "Определить место" button in edit mode | `buildTokens`, `geo:status` |
| Permission requested and pending | `ОПРЕДЕЛЯЮ МЕСТО` | status `requesting` |
| Restart, offline, permission granted | the cached values, with `УСТАРЕЛО`, from the moment the snapshot is restored | `restore()` marks it stale |
| Weather provider unreachable after a fix | the last values, `УСТАРЕЛО` after 20 min, retries on the backoff table | `record_weather_failure` |
| Fix found, weather never arrived | `ПОГОДА НЕДОСТУПНА` | `geo:weather-unavailable` |
| Place lookup failed, no name yet | `НАЗВАНИЕ МЕСТА НЕДОСТУПНО` | `geo:place` |
| Place lookup failed, name known | the last name stays, and its status becomes `unavailable` | `commit_place_name_on_network` |
| Windows positioning failed (not denied) | `МЕСТО НЕДОСТУПНО` if no weather is known, otherwise status `retrying` with no status cell | `record_location_failure` |
| Permission denied | weather removed, cached snapshot deleted, place cache cleared, `ГЕОЛОКАЦИЯ ОТКЛЮЧЕНА` | `set_location_access("denied")` |
| Consent closed, timed out, or WinRT failed | status `unavailable`; the persisted decision still holds | `complete` in `weather_location.rs` |
| Browser reports no network | `НЕТ ПОДКЛЮЧЕНИЯ` and the offline symbol | `buildTokens`, `widget.js` |
| IP providers unreachable | the old indicator, `IP · УСТАРЕЛ` after 30 min | `commit_ip`, the cache |
| A provider refuses (403 or 429) | that provider sits out its cooldown; the chain or the next attempt moves on | `PlaceLookupError::cooldown`, `lookup_ip_provider` |

No failure path blocks the panel. Each network call has the 4-second `HTTP_TIMEOUT`, and DNS resolution runs on its own thread under the same timeout.

---

## Step 13: Shutdown

`MicrographicsModule::shutdown` calls `halt`, which sets the stop flag and hands the thread's join handle to a detached thread named `weather-reaper`. The shutdown call does not wait for the network.

Filename: modules/micrographics/rust/micrographics.rs (abridged)

```rust
fn stop(&self) {
    self.stop.store(true, Ordering::Release);
    let handle = match self.thread.lock() { /* ... take the handle ... */ };
    let Some(handle) = handle else { return };
    handle.thread().unpark();
    thread::Builder::new().name("weather-reaper".to_string())
        .spawn(move || { let _ = handle.join(); }).ok();
}
```

The reason is in the comment on `stop`. `ureq` has no cancellation, so a join could wait as long as one request, several times the 1,500 ms budget the kernel grants a module. The one write that can still follow is a single cache transaction, which leaves the previous value if it is killed mid-commit.

`init` starts no thread in a diagnostic run, where it only reports whether an access record exists, or when the module is disabled. Switching a module at run time is covered by [modules.lifecycle](../reference/modules.md#r-modules.lifecycle).

---

## Tests and checks

`micrographics.rs` holds 59 `#[test]` functions. Their names cover provider order and cooldowns, place-name matching, the Open-Meteo field rules, permission revocation, restart restore, route changes, and a shutdown that does not wait. The file does not say how they are run.

The files under `tests/` run in the real WebView:

- [tests/composition.js](../../modules/micrographics/tests/composition.js) checks 350 randomized layouts, the overlap and dock rules, the attribution labels, and forecast windows across an hour boundary. Its header says it runs through `udtk gui js`.
- [tests/network.js](../../modules/micrographics/tests/network.js) checks that the IP label goes stale without another backend event.
- [tests/live.js](../../modules/micrographics/tests/live.js) checks the live widget in a tight viewport: scrolling, the reachable last value, and a relayout that keeps the nodes and the scroll position.
- [tests/resume.ps1](../../modules/micrographics/tests/resume.ps1) posts the resume message to the session-end window and waits up to 30 seconds for a fresh snapshot. It needs a Windows session and reads through `udtk invoke weather_current`.

---

## Known gaps

- No gap is known. Note that `tests/network.js` imports `/modules/micrographics/composition.js`: the build copies each module's `ui/` folder to `/modules/<id>/`, which is the path the page loads `widget.js` from as well.
