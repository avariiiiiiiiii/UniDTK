//! What a subscription is to this module: the link a person pastes, the request that
//! fetches it with the device headers a provider's limit counts, and the answer split
//! into servers and the provider's notes.

use std::io::Read;
use std::time::Duration;

use serde::Serialize;

use super::{decode_subscription, parse_subscription, VpnServer};

/// The longest subscription body read: a thousand servers are far below it.
const MAX_BODY: u64 = 4 * 1024 * 1024;

/// The longest link the field takes.
const MAX_LINK: usize = 4096;

/// What a provider says beside its servers.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Notes {
    /// The profile's name (`profile-title`).
    pub title: Option<String>,
    /// The provider's announcement (`announce`) and the placeholder entries it sends in
    /// place of servers (a device limit, an unsupported client), one line of text each.
    pub comments: Vec<String>,
    /// Why the provider gave no servers, read from its headers.
    pub refusal: Option<String>,
}

pub struct Fetched {
    pub servers: Vec<VpnServer>,
    pub notes: Notes,
    pub skipped: usize,
}

/// The subscription address a pasted link means. A client's own link carries the
/// address inside it (`happ://add/<url>`, `v2raytun://import/<url>`, `?url=<encoded>`)
/// and is unwrapped; one this module cannot fetch is refused with a reason that does
/// not repeat the link, because the link is a credential.
pub fn normalize_link(text: &str) -> Result<String, String> {
    let link = text.trim();
    if link.is_empty() {
        return Ok(String::new());
    }
    if link.len() > MAX_LINK {
        return Err("Ссылка подписки слишком длинная".into());
    }
    if is_http(link) {
        return checked_http(link);
    }
    let Some((scheme, rest)) = link.split_once("://") else {
        return Err("Нужна ссылка подписки, которая начинается с https://".into());
    };
    let scheme = scheme.to_ascii_lowercase();
    if rest.to_ascii_lowercase().starts_with("crypt") {
        return Err(format!(
            "Зашифрованная ссылка {scheme}:// открывается только в своём приложении. Вставьте обычную ссылку подписки https://…"
        ));
    }
    let inner = embedded_address(rest)
        .ok_or_else(|| format!("В ссылке {scheme}:// нет адреса подписки. Вставьте обычную ссылку https://…"))?;
    if !is_http(&inner) {
        return Err("Ссылка завёрнута дважды. Вставьте обычную ссылку подписки https://…".into());
    }
    checked_http(&inner)
}

fn is_http(link: &str) -> bool {
    let lower = link.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

/// The address an app link carries: the value of `url=` (percent-encoded up to the next
/// parameter, or raw to the end), or the rest of the path when it is itself a link. A
/// fragment (`#name`) is the app's label for the profile, never part of the address. A
/// second app link in front of the address comes back as it is, so the caller refuses it.
fn embedded_address(rest: &str) -> Option<String> {
    let rest = rest.split('#').next().unwrap_or_default();
    if let Some((_, query)) = rest.split_once('?') {
        let pairs: Vec<&str> = query.split('&').collect();
        if let Some(index) = pairs.iter().position(|pair| pair.starts_with("url=")) {
            let value = pairs[index].trim_start_matches("url=");
            return Some(if value.to_ascii_lowercase().contains("%3a") {
                percent_decode(value)
            } else {
                pairs[index..].join("&").trim_start_matches("url=").to_string()
            });
        }
    }
    let lower = rest.to_ascii_lowercase();
    let start = [lower.find("https://"), lower.find("http://")].into_iter().flatten().min()?;
    if rest[..start].contains("://") {
        return Some(rest.to_string());
    }
    Some(rest[start..].to_string())
}

/// `%XX` decoded; a `+` stays a plus, because subscription tokens carry it.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An http(s) address this module will fetch: no user and password in it (a second
/// credential, which errors would print), and not this computer.
fn checked_http(link: &str) -> Result<String, String> {
    let link = link.split('#').next().unwrap_or(link);
    let (_, rest) = link.split_once("://").unwrap_or(("", link));
    let authority = rest.split(['/', '?']).next().unwrap_or_default();
    if authority.contains('@') {
        return Err("В ссылке подписки не должно быть имени и пароля".into());
    }
    let host = host_of(authority);
    if host.is_empty() {
        return Err("В ссылке подписки нет адреса сервера".into());
    }
    if is_this_computer(&host) {
        return Err("Ссылка подписки указывает на этот компьютер".into());
    }
    Ok(link.to_string())
}

/// The host of an authority (`host`, `host:port`, `[v6]:port`), lower case.
fn host_of(authority: &str) -> String {
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or_default()
    } else {
        authority.rsplit_once(':').map_or(authority, |(host, _)| host)
    };
    host.to_ascii_lowercase()
}

/// Loopback, unspecified and link-local addresses: a subscription there would send the
/// device headers to a local service. A home server on the local network is a real
/// subscription source and stays allowed.
fn is_this_computer(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_unspecified() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xffc0) == 0xfe80,
        Err(_) => false,
    }
}

/// The host a subscription address names: the provider its device id is made for.
pub fn provider_host(link: &str) -> String {
    let (_, rest) = link.split_once("://").unwrap_or(("", link));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    host_of(authority.rsplit('@').next().unwrap_or(authority))
}

/// Every URL in a message cut back to its scheme and host: an error quotes the request
/// URL, the token is its path or query, and the message goes to the log (readable
/// through `udtk logs`) and to the panel. A `user:password@` goes too.
pub fn redact_urls(message: &str) -> String {
    message
        .split(' ')
        .map(|word| match word.find("://") {
            Some(scheme_end) => {
                let authority_start = scheme_end + 3;
                let rest = &word[authority_start..];
                let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                let authority = rest[..authority_len].trim_end_matches(':');
                let host = authority.rsplit('@').next().unwrap_or(authority);
                let cut = authority_len < rest.len() || host.len() < authority.len();
                let trailer = if word.ends_with(':') { ":" } else { "" };
                let tail = if cut { "/…" } else { "" };
                format!("{}{host}{tail}{trailer}", &word[..authority_start])
            }
            None => word.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A provider's header as text: plain, or `base64:` and UTF-8 (Happ's convention, which
/// Remnawave follows for `profile-title` and `announce`).
pub fn header_text(value: &str) -> String {
    let value = value.trim();
    let text = match value.strip_prefix("base64:") {
        Some(encoded) => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(encoded.trim())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default()
        }
        None => value.to_string(),
    };
    text.trim().chars().take(300).collect()
}

/// An entry a provider sends in place of a server: it cannot be dialled (`0.0.0.0`, `::`,
/// port 0 or 1) or carries no identity (an all-zero id), and its name is the message.
fn is_placeholder(server: &VpnServer) -> bool {
    let host = server.host.trim_matches(['[', ']']);
    host == "0.0.0.0"
        || host == "::"
        || server.port <= 1
        || (!server.uuid.is_empty() && server.uuid.chars().all(|c| c == '0' || c == '-'))
}

/// The servers, and the placeholders' names as comments, in the subscription's order.
pub fn split_placeholders(entries: Vec<VpnServer>) -> (Vec<VpnServer>, Vec<String>) {
    let mut servers = Vec::new();
    let mut comments = Vec::new();
    for entry in entries {
        if is_placeholder(&entry) {
            if entry.tag != format!("{}:{}", entry.host, entry.port) && !entry.tag.trim().is_empty() {
                comments.push(entry.tag);
            }
        } else {
            servers.push(entry);
        }
    }
    (servers, comments)
}

/// Fetches a subscription with the device headers for its provider. A provider's answer
/// is `Ok` even with no servers (a device limit, placeholders only): only a transport
/// failure is an error, and only then may the caller fall back to a cached list.
pub fn fetch(url: &str) -> Result<Fetched, String> {
    let host = provider_host(url);
    let mut request = ureq::get(url)
        .set("User-Agent", "v2box/1.0")
        .timeout(Duration::from_secs(15));
    for (name, value) in super::hwid::headers(&host) {
        request = request.set(name, &value);
    }
    let response = request
        .call()
        .map_err(|error| redact_urls(&format!("subscription request failed: {error}")))?;
    let flag = |name: &str| response.header(name).is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));
    let refusal = if flag("x-hwid-max-devices-reached") {
        Some("Лимит устройств подписки исчерпан: удалите старое устройство у провайдера".to_string())
    } else if flag("x-hwid-not-supported") {
        Some("Провайдер не принял идентификатор устройства".to_string())
    } else {
        None
    };
    let title = response.header("profile-title").map(header_text).filter(|t| !t.is_empty());
    let announce = response.header("announce").map(header_text).filter(|t| !t.is_empty());

    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut body)
        .map_err(|error| redact_urls(&format!("subscription read failed: {error}")))?;
    if body.len() as u64 > MAX_BODY {
        return Err("subscription is larger than 4 MiB".into());
    }
    let raw = String::from_utf8_lossy(&body);
    let (entries, skipped) = if raw.trim().is_empty() {
        (Vec::new(), 0)
    } else {
        parse_subscription(&decode_subscription(&raw)?)
    };
    let (servers, placeholders) = split_placeholders(entries);
    let mut comments: Vec<String> = announce.into_iter().collect();
    comments.extend(placeholders);
    if servers.is_empty() && comments.is_empty() && refusal.is_none() {
        return Err("no usable VLESS/Trojan servers found in subscription".into());
    }
    Ok(Fetched { servers, notes: Notes { title, comments, refusal }, skipped })
}

#[cfg(test)]
#[path = "subscription_tests.rs"]
mod tests;
