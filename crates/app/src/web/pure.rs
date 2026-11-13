//! Browser-independent rules of the web platform: address building, name cleaning, accept lists.
//!
//! Nothing here touches `web-sys`. The file is also compiled into the tests of `apps/drafftink-web`, which
//! run natively (the machine has no wasm test runner), so keep it free of wasm-only imports.

use drafftink_engine::ports::Ns;

/// `ws(s)://<host><path>` for the server that delivered the page. `protocol` is `location.protocol`
/// (`"https:"`), `host` is `location.host` (with the port). `None` when the page is not http(s) (a `file:`
/// page has no server) or `path` is not an absolute path.
pub(crate) fn same_origin_ws_url(protocol: &str, host: &str, path: &str) -> Option<String> {
    let scheme = match protocol {
        "https:" => "wss",
        "http:" => "ws",
        _ => return None,
    };
    if host.is_empty() || !path.starts_with('/') {
        return None;
    }
    Some(format!("{scheme}://{host}{path}"))
}

/// True when `hostname` (`location.hostname`, IPv6 in brackets) names this machine. Only then may the page
/// talk plain `ws://` to its own server without the user's say-so.
pub(crate) fn is_loopback_host(hostname: &str) -> bool {
    let name = hostname.trim().trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || name == "::1"
        || name.parse::<std::net::Ipv4Addr>().is_ok_and(|ip| ip.is_loopback())
}

/// True for Apple platforms, where `Mod` is Cmd. An iPad with a keyboard counts.
pub(crate) fn is_mac_platform(platform: &str, user_agent: &str) -> bool {
    let platform = platform.to_ascii_lowercase();
    platform.starts_with("mac")
        || platform.starts_with("iphone")
        || platform.starts_with("ipad")
        || user_agent.contains("Macintosh")
        || user_agent.contains("iPhone")
        || user_agent.contains("iPad")
}

/// The longest download name kept, in characters.
const MAX_NAME_CHARS: usize = 120;

/// A file name that is safe to hand to the browser: no path separators, no control characters, never empty.
pub(crate) fn download_name(name: &str) -> String {
    let cleaned: String =
        name.chars()
            .map(|c| {
                if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                    '_'
                } else {
                    c
                }
            })
            .take(MAX_NAME_CHARS)
            .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_owned();
    if cleaned.is_empty() { "Untitled.drafftink".to_owned() } else { cleaned }
}

/// The `accept` attribute of the file input that picks a document.
pub(crate) const ACCEPT_DOCUMENT: &str = ".drafftink,.json,.excalidraw,.png,application/json,image/png";
/// The `accept` attribute of the file input that picks pictures.
pub(crate) const ACCEPT_IMAGES: &str = "image/png,image/jpeg,image/webp,image/gif,image/bmp,image/x-icon,image/vnd.microsoft.icon,.png,.jpg,.jpeg,.webp,.gif,.bmp,.ico";
/// The `accept` attribute of the file input that picks a library.
pub(crate) const ACCEPT_LIBRARY: &str = ".excalidrawlib,.json,application/json";

/// Files above this size are not read into memory (the same cap as dropped files on the desktop).
pub(crate) const MAX_PICKED_BYTES: f64 = 256.0 * 1024.0 * 1024.0;

/// The name of a namespace in the IndexedDB records.
pub(crate) fn ns_name(ns: Ns) -> &'static str {
    match ns {
        Ns::Documents => "documents",
        Ns::Autosave => "autosave",
        Ns::Settings => "settings",
        Ns::Library => "library",
    }
}

/// The namespace stored under `name`, if any.
pub(crate) fn ns_from_name(name: &str) -> Option<Ns> {
    [Ns::Documents, Ns::Autosave, Ns::Settings, Ns::Library].into_iter().find(|ns| ns_name(*ns) == name)
}

/// The IndexedDB key of one entry: `<namespace>/<key>`.
pub(crate) fn record_key(ns: Ns, key: &str) -> String {
    format!("{}/{key}", ns_name(ns))
}

/// Finished results that wait for the call that asked, newest last. The browser draws one picture at a time and
/// the engine asks again for the same request, so a `batch` with two exports needs both pictures to be kept:
/// with one slot the second drawing would throw the first away and the batch would never complete.
///
/// A good result stays on the shelf and can be fetched again; a failure is handed out once, so a transient error
/// is not remembered for the same request. The oldest entry goes when the shelf is full.
pub(crate) struct Shelf<K, V> {
    items: std::collections::VecDeque<(K, Result<V, String>)>,
    capacity: usize,
}

impl<K: PartialEq, V: Clone> Shelf<K, V> {
    /// An empty shelf that holds at most `capacity` results (at least one).
    pub(crate) fn new(capacity: usize) -> Self {
        Self { items: std::collections::VecDeque::new(), capacity: capacity.max(1) }
    }

    /// Puts a finished result on the shelf, replacing an older one for the same key.
    pub(crate) fn put(&mut self, key: K, result: Result<V, String>) {
        self.items.retain(|(k, _)| *k != key);
        if self.items.len() >= self.capacity {
            self.items.pop_front();
        }
        self.items.push_back((key, result));
    }

    /// The result for `key`, if there is one. A failure is removed by this call.
    pub(crate) fn get(&mut self, key: &K) -> Option<Result<V, String>> {
        let at = self.items.iter().position(|(k, _)| k == key)?;
        match &self.items.get(at)?.1 {
            Ok(v) => Some(Ok(v.clone())),
            Err(_) => self.items.remove(at).map(|(_, r)| r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_follows_the_page_protocol() {
        assert_eq!(
            same_origin_ws_url("https:", "draw.example.com", "/ws").as_deref(),
            Some("wss://draw.example.com/ws")
        );
        assert_eq!(same_origin_ws_url("http:", "127.0.0.1:3030", "/ws").as_deref(), Some("ws://127.0.0.1:3030/ws"));
        assert_eq!(same_origin_ws_url("file:", "", "/ws"), None);
        assert_eq!(same_origin_ws_url("https:", "a.b", "ws"), None);
        assert_eq!(same_origin_ws_url("https:", "", "/ws"), None);
    }

    #[test]
    fn loopback_hosts_are_this_machine_only() {
        for h in ["localhost", "LOCALHOST", "127.0.0.1", "127.9.9.9", "[::1]", "::1", "app.localhost"] {
            assert!(is_loopback_host(h), "{h}");
        }
        for h in ["example.com", "192.168.1.5", "localhost.evil.com", "10.0.0.1", "", "127.0.0.1.evil.com"] {
            assert!(!is_loopback_host(h), "{h}");
        }
    }

    #[test]
    fn mac_detection() {
        assert!(is_mac_platform("MacIntel", ""));
        assert!(is_mac_platform("iPad", ""));
        assert!(is_mac_platform("", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"));
        assert!(!is_mac_platform("Linux x86_64", "Mozilla/5.0 (X11; Linux x86_64)"));
        assert!(!is_mac_platform("Win32", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)"));
    }

    #[test]
    fn download_names_are_cleaned() {
        assert_eq!(download_name("Plan.drafftink"), "Plan.drafftink");
        assert_eq!(download_name("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(download_name("a\\b:c*d?.png"), "a_b_c_d_.png");
        assert_eq!(download_name("  "), "Untitled.drafftink");
        assert_eq!(download_name(".hidden"), "hidden");
        assert_eq!(download_name("tab\there\n.json"), "tab_here_.json");
        assert_eq!(download_name(&"x".repeat(500)).chars().count(), MAX_NAME_CHARS);
        assert_eq!(download_name("Réunion – été.drafftink"), "Réunion – été.drafftink");
    }

    #[test]
    fn accept_lists_name_extensions_and_types() {
        assert!(ACCEPT_DOCUMENT.contains(".drafftink") && ACCEPT_DOCUMENT.contains(".excalidraw"));
        assert!(ACCEPT_IMAGES.contains("image/png") && ACCEPT_IMAGES.contains(".webp"));
        assert!(ACCEPT_IMAGES.contains(".gif") && ACCEPT_IMAGES.contains(".bmp") && ACCEPT_IMAGES.contains(".ico"));
        assert!(ACCEPT_LIBRARY.contains(".excalidrawlib"));
    }

    #[test]
    fn namespaces_round_trip() {
        for ns in [Ns::Documents, Ns::Autosave, Ns::Settings, Ns::Library] {
            assert_eq!(ns_from_name(ns_name(ns)), Some(ns));
        }
        assert_eq!(ns_from_name("nope"), None);
        assert_eq!(record_key(Ns::Autosave, "slot/1"), "autosave/slot/1");
    }

    #[test]
    fn a_shelf_keeps_finished_pictures_so_a_batch_with_two_exports_converges() {
        // Each rerun of the batch asks for picture 1, then picture 2. One drawing runs at a time.
        let mut shelf: Shelf<u32, Vec<u8>> = Shelf::new(4);
        let mut drawing: Option<u32> = None;
        let mut runs = 0;
        loop {
            runs += 1;
            assert!(runs < 10, "the batch never converges");
            let mut waiting = false;
            for key in [1, 2] {
                if shelf.get(&key).is_some() {
                    continue;
                }
                waiting = true;
                if drawing.is_none() {
                    drawing = Some(key);
                }
                break;
            }
            if !waiting {
                break;
            }
            // The drawing ends before the next rerun.
            if let Some(key) = drawing.take() {
                shelf.put(key, Ok(vec![key as u8]));
            }
        }
        assert_eq!(runs, 3, "one rerun per picture, then the batch completes");
    }

    #[test]
    fn a_shelf_hands_out_a_good_picture_again_but_a_failure_only_once() {
        let mut shelf: Shelf<&str, u8> = Shelf::new(2);
        shelf.put("a", Ok(1));
        shelf.put("b", Err("lost device".to_owned()));
        assert_eq!(shelf.get(&"a"), Some(Ok(1)));
        assert_eq!(shelf.get(&"a"), Some(Ok(1)), "the same picture can be asked for again");
        assert_eq!(shelf.get(&"b"), Some(Err("lost device".to_owned())));
        assert_eq!(shelf.get(&"b"), None, "a failure is not remembered: the next ask draws again");
    }

    #[test]
    fn a_shelf_drops_the_oldest_picture_when_full() {
        let mut shelf: Shelf<u8, u8> = Shelf::new(2);
        shelf.put(1, Ok(1));
        shelf.put(2, Ok(2));
        shelf.put(3, Ok(3));
        assert_eq!(shelf.get(&1), None);
        assert_eq!(shelf.get(&2), Some(Ok(2)));
        assert_eq!(shelf.get(&3), Some(Ok(3)));
        shelf.put(3, Ok(4));
        assert_eq!(shelf.get(&3), Some(Ok(4)), "a new picture of the same request replaces the old one");
    }
}
