# Configuring Happ Desktop for Windows (Russia / Bypassing Blocks)

This document is the full guide for configuring the **Happ Desktop** client on Windows, for stable operation under blocking and TSPU (Technical Means of Countering Threats, Russia's DPI equipment) with split tunneling.

> The Happ interface is in Russian. Labels are given in English, and the original Russian label is shown in parentheses in each path.

---

## 1. Architecture concept (Split Tunneling)

The main goal of the configuration is to separate the network flows:
1. **Direct traffic (Direct / Bypass):** Russian resources (banks, Gosuslugi government services, marketplaces Ozon/WB, Yandex services, VK, Kinopoisk, local networks) go directly through the home ISP at maximum speed, with no blocking based on a foreign IP.
2. **Proxied traffic (Proxy):** blocked or throttled services (YouTube, Discord, Instagram, X/Twitter, foreign resources) are routed through the protected tunnel (VLESS / Reality / Trojan / Shadowsocks).

---

## 2. System requirements and TUN mode

### Important requirement
* **Run as administrator:** the Happ client creates a virtual system network adapter (Wintun). Installing it and switching routes requires elevated privileges. In the properties of the `happ.exe` shortcut (*Compatibility* tab), enable *"Run this program as an administrator"*.

### Adapter and network mode settings
* **System proxy:** `Off` *(must not run in parallel with TUN, to avoid conflicts)*.
* **TUN:** `On` *(intercepts all TCP/UDP traffic of the system, including games and consoles)*.
* **TUN provider:** `sing-box` *(provides high performance and minimal latency)*.
* **TUN mode:** `Mixed`.
* **TUN configuration:** `Default`.
* **MTU:** `1400` is recommended (if connections drop when large packets are transferred).

---

## 3. Routing rules (Routing / Direct RU)

Path in the interface:  
`Settings` (Настройки) → `Routing rules` (Правила маршрутизации) → `geo` → `Edit rules` (Редактировать правила) → `Direct`

### Rule list for the Direct group
All items from the list below must be present in the **Direct URL or IP** field:

```text
# Local subnets (LAN)
10.0.0.0/8
172.16.0.0/12
192.168.0.0/16
169.254.0.0/16
224.0.0.0/4
255.255.255.255

# Russian IP ranges and services
geoip:ru
geosite:category-ru
geosite:ru

# Russian domain zone masks
regexp:.*\.ru$
regexp:.*\.рф$
```

### Explanation of the rules
* `geoip:ru` — the range of all IP addresses registered in Russia.
* `geosite:category-ru` — a preset list of domains of Russian services (banks, marketplaces, government services, streaming).
* `geosite:ru` — the domain database for the `.ru` zone.
* `regexp:.*\.ru$` and `regexp:.*\.рф$` — regular expressions for direct routing of any domain under the top-level `.ru` and `.рф` zones.
* Final action (Default Outbound) for unmatched traffic: **`Proxy`**.

---

## 4. DNS configuration

Path in the interface:  
`Settings` (Настройки) → `Routing rules` (Правила маршрутизации) → `geo`

### Domain settings
* **Enable Fake DNS:** `Off`.
* **Domain strategy:** `IPIfNonMatch`  
  *(The client first matches the domain name by geosite. If the domain is not found, the IP is resolved, and the address is then checked against the geoip rules. This is the optimal mode)*.

### Remote DNS (for traffic through the VPN)
* **Remote DNS type:** `DoH` (DNS-over-HTTPS)
* **URL:** `https://cloudflare-dns.com/dns-query` (or `https://1.1.1.1/dns-query`)
* **IP:** `1.1.1.1`
* *Purpose:* all requests to foreign sites are encrypted and pass inside the tunnel, which rules out tampering by the provider.

### Local DNS (for direct Russian sites)
* **Local DNS type:** `DoH` or `UDP`
* **URL:** `https://dns.yandex.ru/dns-query`
* **IP:** `77.88.8.8`
* *Important:* do not use Google DNS (`8.8.8.8`) for local traffic in Russia: it often returns non-optimal foreign CDN nodes and may be blocked by internal services.

### Resolving settings in "Other settings" (Другие настройки)
* **Use local DNS:** `On`
* **Use DNS from JSON:** `Off`
* **Enable server resolving:** `On`
* **DNS IP for server resolving:** `77.88.8.8` *(guarantees that the IP address of the VPN server is determined successfully when the program starts)*.
* **Enable DNS for TUN:** `On`
* **DNS address for TUN:** `1.1.1.1`

---

## 5. Advanced core parameters (Anti-DPI / Xray)

Path in the interface:  
`Settings` (Настройки) → `Advanced settings` (Расширенные настройки)

| Parameter | Value | Rationale |
| :--- | :--- | :--- |
| **Enable fragmentation** | `On` | Splits the first TLS handshake packet to bypass SNI filtering on TSPU hardware. |
| **Type** | `Xray` | The standard fragment handler. |
| **Fragment packets** | `tlshello` | Splits only the `ClientHello` packet. |
| **Fragment length (min-max)** | `50-100` | Optimal chunk size of the packet. |
| **Fragment interval** | `10-20` | Delay in ms between sending the parts (prevents TSPU from reassembling the packet on the fly). |
| **Max fragment split** | `100-200` | Split boundary. |
| **Enable noise** | `Off` | Requires server-side support. Without support it breaks the handshake. |
| **Enable multiplexer (Mux)** | `Off` | **Strictly off.** Mux worsens the ping for UDP (Discord, games) and cuts video speed. |
| **Preferred IP type** | `IPv4` | Avoids delays from attempts to reach non-working IPv6 routes. |
| **Packet sniffing** | `On` | Required to recognize domains inside TUN traffic and for geosite to work. |

---

## 6. Applying changes and verification

After saving the parameters, do the following:
1. On the Happ main screen, click **Disconnect**.
2. Click **Connect** to rebuild the virtual adapter and the routing table.
3. Restart the web browser (to reset the local socket and DNS cache).

### Verification:
* **Russian segment:** open `2ip.ru` — your real IP and the Russian ISP should be displayed. Gosuslugi and bank sites should open without a captcha.
* **Foreign segment:** open YouTube, Discord, foreign services — traffic should go through the IP of your VPN server.

---

## 7. Troubleshooting

* **TUN fails to start / adapter is not created:**
  * Run Happ as administrator.
  * Check whether the antivirus / Windows Defender is blocking the creation of the Wintun virtual network device.
* **Conflicts with GoodbyeDPI / Zapret / ByeDPI:**
  * Do not run Happ in TUN mode at the same time as utilities based on the WinDivert driver. Choose one solution.
* **Discord voice does not work:**
  * Make sure **TUN** mode is enabled (Discord voice traffic goes over UDP, and a regular system proxy does not intercept it).
