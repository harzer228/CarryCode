use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    io::Read,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use url::Url;

pub const LOCAL_SEARX: &str = "http://127.0.0.1:8080";
pub const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    pub searx: Option<String>,
    pub brave_key: Option<String>,
}

impl Settings {
    pub fn searx(url: String) -> Self {
        Self {
            searx: Some(url),
            brave_key: None,
        }
    }
}

fn client() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(3))
        .timeout(TIMEOUT)
}

pub fn parse_searx(input: &str) -> Result<String> {
    super::endpoint(input)
}

pub fn search(settings: &Settings, query: &str) -> Result<String> {
    ensure!(
        !query.trim().is_empty()
            && query.chars().count() <= 500
            && !query.contains(['\n', '\r', '\0']),
        "Expected a single search query of up to 500 characters"
    );
    let agent = client().build();
    let (response, searx) = if let Some(base) = &settings.searx {
        (
            agent
                .get(&format!("{base}/search"))
                .query("q", query)
                .query("format", "json")
                .call(),
            true,
        )
    } else if let Some(key) = &settings.brave_key {
        (
            agent
                .get("https://api.search.brave.com/res/v1/llm/context")
                .query("q", query)
                .query("maximum_number_of_urls", "8")
                .set("X-Subscription-Token", key)
                .call(),
            false,
        )
    } else {
        bail!(tr!(
            "Выбери SearXNG или Brave Search в меню /search.",
            "Select SearXNG or Brave Search from the /search menu."
        ));
    };
    let response = response.map_err(|error| match error {
        ureq::Error::Status(403, _) if searx => anyhow::anyhow!(tr!(
            "SearXNG запретил запрос (HTTP 403). Проверь доступ и формат json в search.formats.",
            "SearXNG denied the request (HTTP 403). Check access and enable json in search.formats."
        )),
        ureq::Error::Transport(_) if settings.searx.as_deref() == Some(LOCAL_SEARX) => anyhow::anyhow!(tr!(
            "SearXNG не отвечает на 127.0.0.1:8080. Запусти локальный сервер или выбери другой сервис в /search.",
            "SearXNG is not responding at 127.0.0.1:8080. Start the local server or choose another service in /search."
        )),
        other => network_error(other),
    })?;
    let body = bounded(response, 512 * 1024)?;
    let value: Value = serde_json::from_str(&body).context("Search service did not return JSON")?;
    format_results(&value, searx)
}

fn format_results(value: &Value, searx: bool) -> Result<String> {
    let results = if searx {
        &value["results"]
    } else {
        &value["grounding"]["generic"]
    };
    let items = results
        .as_array()
        .context("Unexpected search result format")?;
    let mut output = Vec::new();
    for item in items.iter().take(8) {
        let Some(address) = item["url"].as_str() else {
            continue;
        };
        if public_url(address).is_err() {
            continue;
        }
        let title = item["title"].as_str().unwrap_or(address);
        let snippets = if searx {
            item["content"].as_str().unwrap_or("").to_owned()
        } else {
            item["snippets"]
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default()
        };
        output.push(format!(
            "{}\n{}\n{}",
            clean(title, 220),
            address,
            clean(&snippets, 1000)
        ));
    }
    if !output.is_empty() {
        return Ok(output.join("\n\n"));
    }
    if searx && let Some(engines) = value["unresponsive_engines"].as_array() {
        let failed: Vec<_> = engines
            .iter()
            .filter_map(|engine| engine[0].as_str().map(|name| clean(name, 40)))
            .take(5)
            .collect();
        if !failed.is_empty() {
            bail!(trf!(
                "SearXNG ответил, но результатов нет. Ошибки поисковых движков: {}. Проверь соединение сервера с интернетом или его настройки.",
                "SearXNG responded with no results. Search engines reported errors: {}. Check the server's internet access or settings.",
                failed.join(", ")
            ));
        }
    }
    Ok(tr!("Результатов нет.", "No search results.").into())
}

fn network_error(error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(status, _) => anyhow::anyhow!("HTTP {status}"),
        ureq::Error::Transport(error) => {
            anyhow::anyhow!("Network request failed ({})", error.kind())
        }
    }
}

fn clean(text: &str, count: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(count)
        .collect()
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, _, _] = v4.octets();
            a > 0
                && a < 224
                && !v4.is_private()
                && !v4.is_loopback()
                && !v4.is_link_local()
                && !v4.is_documentation()
                && !(a == 100 && (64..128).contains(&b))
                && !(a == 198 && matches!(b, 18 | 19))
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return public_ip(mapped.into());
            }
            v6.segments()[0] & 0xe000 == 0x2000
                && v6.segments()[0] != 0x2002
                && !(v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8)
        }
    }
}

fn public_url(input: &str) -> Result<Url> {
    let url = Url::parse(input)?;
    ensure!(
        matches!(url.scheme(), "https" | "http")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none(),
        "Expected a public HTTP(S) link"
    );
    match url.host().context("URL needs a host")? {
        url::Host::Domain(name) => ensure!(
            name.contains('.') && !name.ends_with(".local") && !name.ends_with(".localhost"),
            "Expected a public hostname"
        ),
        url::Host::Ipv4(ip) => ensure!(public_ip(ip.into()), "Expected a public address"),
        url::Host::Ipv6(ip) => ensure!(public_ip(ip.into()), "Expected a public address"),
    }
    Ok(url)
}

fn public_resolver(host: &str) -> std::io::Result<Vec<SocketAddr>> {
    let addresses: Vec<_> = host.to_socket_addrs()?.collect();
    if addresses.is_empty() || addresses.iter().any(|address| !public_ip(address.ip())) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Expected a public address",
        ));
    }
    Ok(addresses)
}

pub fn fetch(address: &str, cancel: &AtomicBool) -> Result<String> {
    let mut url = public_url(address)?;
    let agent = client().resolver(public_resolver).build();
    let started = Instant::now();
    for _ in 0..4 {
        let remaining = TIMEOUT.saturating_sub(started.elapsed());
        ensure!(
            !cancel.load(Ordering::Relaxed) && !remaining.is_zero(),
            "Page request stopped"
        );
        let response = agent
            .get(url.as_str())
            .timeout(remaining)
            .set("Accept", "text/html,text/plain,text/markdown")
            .call()
            .map_err(network_error)?;
        if (300..400).contains(&response.status()) {
            let next = url.join(
                response
                    .header("location")
                    .context("Redirect has no destination")?,
            )?;
            url = public_url(next.as_str())?;
            continue;
        }
        let kind = response
            .header("Content-Type")
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        ensure!(
            matches!(kind.as_str(), "text/html" | "text/plain" | "text/markdown"),
            "This URL did not return a supported text page"
        );
        let body = bounded(response, 1024 * 1024)?;
        let body = if kind == "text/html" {
            html_text(&body)
        } else {
            body
        };
        let mut text = format!("Source: {url}\n");
        let mut chars = body
            .chars()
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'));
        text.extend(chars.by_ref().take(16000));
        if chars.next().is_some() {
            text.push_str("\n[page text truncated]");
        }
        return Ok(text);
    }
    bail!("Too many redirects")
}

fn bounded(response: ureq::Response, limit: u64) -> Result<String> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Response exceeds the download limit"
    );
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn html_text(html: &str) -> String {
    let mut text = String::new();
    let mut rest = html;
    let mut hidden = None;
    while let Some((before, suffix)) = rest.split_once('<') {
        if hidden.is_none() {
            text.push_str(before);
        }
        let Some((tag, tail)) = suffix.split_once('>') else {
            break;
        };
        rest = tail;
        let tag = tag
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if let Some(name) = hidden {
            if tag.strip_prefix('/') == Some(name) {
                hidden = None;
            }
        } else if matches!(tag.as_str(), "script" | "style" | "noscript") {
            hidden = Some(match tag.as_str() {
                "script" => "script",
                "style" => "style",
                _ => "noscript",
            });
        } else {
            text.push(' ');
        }
    }
    if hidden.is_none() {
        text.push_str(rest);
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_searx_and_brave_result_shapes() -> Result<()> {
        let searx = serde_json::json!({"results": [
            {"url":"https://docs.rs","title":"Docs.rs","content":"Rust docs"},
            {"url":"http://127.0.0.1/","title":"Private","content":"not a public result"}
        ]});
        let formatted = format_results(&searx, true)?;
        assert!(formatted.contains("Docs.rs\nhttps://docs.rs\nRust docs"));
        assert!(!formatted.contains("Private"));
        let brave = serde_json::json!({"grounding":{"generic":[{"url":"https://doc.rust-lang.org","title":"Rust","snippets":["Reference","Library"]}]}});
        assert!(format_results(&brave, false)?.contains("Reference Library"));
        Ok(())
    }

    #[test]
    fn empty_searx_results_report_upstream_errors() {
        let value = serde_json::json!({
            "results": [], "unresponsive_engines": [["duckduckgo", "timeout"]]
        });
        let error = format_results(&value, true).unwrap_err();
        assert!(error.to_string().contains("duckduckgo"));
        assert!(format_results(&serde_json::json!({"results": []}), true).is_ok());
    }

    #[test]
    fn fetch_rejects_local_destinations_and_drops_inline_scripts() {
        for url in [
            "http://localhost/",
            "http://127.1/",
            "http://[::ffff:127.0.0.1]/",
            "file:///test",
            "http://169.254.169.254/",
            "http://name:secret@example.com/",
        ] {
            assert!(public_url(url).is_err(), "{url}");
        }
        assert_eq!(
            html_text("<p>Hello</p><script>private</script><p>&lt;world&gt;</p>"),
            "Hello <world>"
        );
    }
}
