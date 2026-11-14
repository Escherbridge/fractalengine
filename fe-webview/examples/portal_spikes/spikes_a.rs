//! Spikes SP-1..SP-4 (protocol.md §10, §12); see `fe-webview/src/portal_bundle/AGENTS.md` §spikes.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use super::{Harness, Outcome, Spike};

/// The spikes this file runs; EXT-07's `spikes_b::SPIKES` joins them in `TABLE`.
pub const SPIKES: &[Spike] = &[
    Spike {
        id: "SP-1",
        question: "Does interception win for https://fe-portal.<label>.localhost, so a miss never dials loopback?",
        run: sp1_interception,
    },
    Spike {
        id: "SP-2",
        question: "Does wry's navigation callback see sub-frame navigations?",
        run: sp2_frame_navigation,
    },
    Spike {
        id: "SP-3",
        question: "How is PostWebMessageAsJson ordered against NavigationStarting?",
        run: sp3_post_ordering,
    },
    Spike {
        id: "SP-4",
        question: "Do service-worker requests reach the handler and fail registration?",
        run: sp4_service_worker,
    },
];

/// `web/app.js` of bundle A: `?spike=` picks the behaviour; every result goes back as a page report.
pub const BUNDLE_SCRIPT: &str = r#"
const query = new URLSearchParams(location.search);
const spike = query.get('spike');
const report = (id, extra) => fetch('/__spike/report?id=' + id + (extra ? '&' + extra : ''));
if (spike === 'sp3') { report('sp3a'); }
if (spike === 'sp2') {
  const blocked = [];
  document.addEventListener('securitypolicyviolation', (event) => blocked.push(event.blockedURI));
  for (const source of ['https://example.org/', 'https://example.com/']) {
    const frame = document.createElement('iframe');
    frame.src = source;
    document.body.appendChild(frame);
  }
  setTimeout(() => report('sp2done', 'blocked=' + encodeURIComponent(blocked.join(','))), 4000);
}
if (spike === 'sp4') {
  if (!navigator.serviceWorker) { report('sp4reg', 'result=unavailable'); }
  else {
    navigator.serviceWorker.register('/sw.js').then(
      () => report('sp4reg', 'result=registered'),
      (error) => report('sp4reg', 'result=rejected&name=' + encodeURIComponent(error.name)));
  }
}
if (spike === 'sp4after') {
  const controlled = navigator.serviceWorker && navigator.serviceWorker.controller;
  report('sp4ctl', 'controller=' + (controlled ? 'set' : 'null'));
}
"#;

/// SP-3's page B: counts every `chrome.webview` message it receives and reports the count.
pub const PAGE_B: &str = r#"<!doctype html><title>spike B</title><script>
const received = [];
const bridge = window.chrome && window.chrome.webview;
if (bridge) { bridge.addEventListener('message', (event) => received.push(event.data && event.data.seq)); }
setTimeout(() => fetch('/__spike/report?id=sp3b&received=' + received.length
  + '&bridge=' + (bridge ? 'yes' : 'no') + '&first=' + (received.length ? received[0] : '')), 1500);
</script>"#;

/// SP-1's external probe: a `no-cors` fetch of two `fe-portal` hosts, then a report.
pub const PAGE_PROBE: &str = r#"<!doctype html><title>spike probe</title><script>
const query = new URLSearchParams(location.search);
Promise.allSettled([
  fetch(query.get('unmounted'), { mode: 'no-cors' }),
  fetch(query.get('example'), { mode: 'no-cors' })
]).then((results) => fetch('/__spike/report?id=sp1probe&unmounted=' + results[0].status
  + '&example=' + results[1].status));
</script>"#;

const LOAD_WAIT: Duration = Duration::from_secs(10);

fn encode(raw: &str) -> String {
    url::form_urlencoded::byte_serialize(raw.as_bytes()).collect()
}

fn failed(observed: impl Into<String>, notes: impl Into<String>) -> Outcome {
    Outcome { observed: observed.into(), pass: false, notes: notes.into() }
}

/// Listeners on loopback port 443 that count every accepted connection.
struct LoopbackWatch {
    stop: Arc<AtomicBool>,
    hits: Arc<AtomicUsize>,
    bound: Vec<&'static str>,
    failures: Vec<String>,
    threads: Vec<JoinHandle<()>>,
}

impl LoopbackWatch {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let hits = Arc::new(AtomicUsize::new(0));
        let mut watch = Self { stop, hits, bound: Vec::new(), failures: Vec::new(), threads: Vec::new() };
        for address in ["127.0.0.1:443", "[::1]:443"] {
            match TcpListener::bind(address) {
                Ok(listener) => {
                    if let Err(error) = listener.set_nonblocking(true) {
                        watch.failures.push(format!("{address}: {error}"));
                        continue;
                    }
                    let stop = watch.stop.clone();
                    let hits = watch.hits.clone();
                    watch.threads.push(std::thread::spawn(move || {
                        while !stop.load(Ordering::SeqCst) {
                            match listener.accept() {
                                Ok(_) => {
                                    hits.fetch_add(1, Ordering::SeqCst);
                                }
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                    std::thread::sleep(Duration::from_millis(10));
                                }
                                Err(_) => break,
                            }
                        }
                    }));
                    watch.bound.push(address);
                }
                Err(error) => watch.failures.push(format!("{address}: {error}")),
            }
        }
        watch
    }

    fn connections(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// The IPv4 listener is the proof; without it nothing can be concluded.
    fn proves_ipv4(&self) -> bool {
        self.bound.contains(&"127.0.0.1:443")
    }
}

impl Drop for LoopbackWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// SP-1: both a miss on an unmounted label and a foreign `fe-portal.*` host are answered by the handler, never dialled.
fn sp1_interception(harness: &mut Harness) -> Outcome {
    let watch = LoopbackWatch::start();
    if !watch.proves_ipv4() {
        return failed(
            "could not bind 127.0.0.1:443",
            format!("a loopback listener is the evidence; free the port and rerun ({:?})", watch.failures),
        );
    }
    let unmounted_url = format!("https://fe-portal.{}.localhost/", harness.label_unmounted);
    let example_url = "https://fe-portal.example.com/";
    let label = harness.label_unmounted.as_str().to_owned();
    let seen_unmounted = |harness: &Harness| {
        harness.handler_requests().iter().filter(|record| record.uri.contains(&label)).count()
    };

    if let Err(error) = harness.load_ungated(&unmounted_url) {
        return failed("load of the unmounted label failed", format!("{error:#}"));
    }
    harness.wait_until(Duration::from_secs(8), |harness| seen_unmounted(harness) >= 1);
    let navigation_hits = seen_unmounted(harness);

    let probe = format!(
        "/probe.html?unmounted={}&example={}",
        encode(&unmounted_url),
        encode(example_url)
    );
    if let Err(error) = harness.load_spike_page(&probe) {
        return failed("probe page load failed", format!("{error:#}"));
    }
    let probe_report = harness.wait_report("sp1probe", Duration::from_secs(15));
    harness.pump_for(Duration::from_secs(1));

    let requests = harness.handler_requests();
    let fetch_hits = seen_unmounted(harness).saturating_sub(navigation_hits);
    let example_hits = requests.iter().filter(|record| record.uri.contains("fe-portal.example.com")).count();
    let statuses: Vec<u16> = requests
        .iter()
        .filter(|record| record.uri.contains(&label) || record.uri.contains("fe-portal.example.com"))
        .map(|record| record.status)
        .collect();
    let connections = watch.connections();
    let pass = navigation_hits >= 1 && fetch_hits >= 1 && example_hits >= 1 && connections == 0 && probe_report.is_some();
    Outcome {
        observed: format!(
            "handler saw unmounted navigation x{navigation_hits}, unmounted fetch x{fetch_hits}, example.com fetch x{example_hits} \
             (statuses {statuses:?}); loopback :443 accepts = {connections}; probe page reported = {}",
            probe_report.is_some()
        ),
        pass,
        notes: format!(
            "listeners {:?}, bind failures {:?}; DNS packets are not captured here: run `pktmon` on port 53 beside this \
             spike (protocol.md §12.5) to complete the zero-DNS half",
            watch.bound, watch.failures
        ),
    }
}

/// SP-2: record whether wry's callback and the native frame handler see the two embedded frames.
fn sp2_frame_navigation(harness: &mut Harness) -> Outcome {
    if let Err(error) = harness.enter_bundle("spike=sp2") {
        return failed("bundle A did not load", format!("{error:#}"));
    }
    let Some(report) = harness.wait_report("sp2done", Duration::from_secs(20)) else {
        return failed("bundle A never reported", "the page did not finish embedding frames");
    };
    let blocked = report.get("blocked").cloned().unwrap_or_default();
    let wry_frames: Vec<String> =
        harness.wry_navigation_urls().into_iter().filter(|url| url.contains("example.")).collect();
    let native_frames: Vec<String> =
        harness.frame_navigation_urls().into_iter().filter(|url| url.contains("example.")).collect();
    let undeclared_blocked = blocked.contains("example.com");
    let declared_blocked = blocked.contains("example.org");
    Outcome {
        observed: format!(
            "wry callback saw frame URLs {wry_frames:?}; native FrameNavigationStarting saw {native_frames:?}; \
             CSP violations for {blocked:?}"
        ),
        pass: undeclared_blocked && !declared_blocked,
        notes: if wry_frames.is_empty() {
            "wry does not report sub-frame navigations: protocol.md §6.6's frame rule must not rely on the callback".into()
        } else {
            "wry reports sub-frame navigations as bare URLs with no frame flag: the §6.6 frame rule needs the native handler".into()
        },
    }
}

/// SP-3: posts queued before NavigationStarting must never reach the next document.
fn sp3_post_ordering(harness: &mut Harness) -> Outcome {
    const POSTS: u32 = 1000;
    const NAVIGATE_TO_B: &str = "location.href = 'https://fe-spike.localhost/b.html'";
    let mut rows: Vec<String> = Vec::new();
    let mut pass = true;
    for inside_callback in [false, true] {
        harness.reset_posts();
        harness.clear_reports();
        if let Err(error) = harness.enter_bundle("spike=sp3") {
            return failed("bundle A did not load", format!("{error:#}"));
        }
        if harness.wait_report("sp3a", LOAD_WAIT).is_none() {
            return failed("bundle A never reported ready", "the page did not load within 10 s");
        }
        if inside_callback {
            harness.post_inside_navigation_callback(POSTS);
            if let Err(error) = harness.eval(NAVIGATE_TO_B) {
                return failed("could not start the navigation", format!("{error:#}"));
            }
            harness.wait_until(Duration::from_secs(10), |harness| harness.navigation_started());
        } else {
            for index in 0..POSTS {
                if harness.navigation_started() {
                    break;
                }
                harness.post_numbered();
                if index == POSTS / 5 {
                    if let Err(error) = harness.eval(NAVIGATE_TO_B) {
                        return failed("could not start the navigation", format!("{error:#}"));
                    }
                }
                harness.pump_once();
            }
            harness.wait_until(Duration::from_secs(5), |harness| harness.navigation_started());
        }
        let posted = harness.posted();
        let started = harness.navigation_started();
        let received = harness
            .wait_report("sp3b", LOAD_WAIT)
            .map(|report| report.get("received").and_then(|count| count.parse::<u32>().ok()).unwrap_or(u32::MAX));
        let label = if inside_callback { "posted inside NavigationStarting" } else { "posted while navigating" };
        match received {
            Some(count) => {
                pass &= started && count == 0;
                rows.push(format!("{label}: {posted} posted before the callback finished, B received {count}"));
            }
            None => {
                pass = false;
                rows.push(format!("{label}: {posted} posted, page B never reported (navigation started = {started})"));
            }
        }
    }
    Outcome {
        observed: rows.join("; "),
        pass,
        notes: "B is served from the harness scheme fe-spike as a stand-in for a declared origin (the production gate \
                admits only public https), so the gate is not consulted for A to B; the ordering question is independent of it"
            .into(),
    }
}

/// SP-4: the worker script request carries `Service-Worker: script`, is answered 403, and no worker controls the page.
fn sp4_service_worker(harness: &mut Harness) -> Outcome {
    if let Err(error) = harness.enter_bundle("spike=sp4") {
        return failed("bundle A did not load", format!("{error:#}"));
    }
    let Some(registration) = harness.wait_report("sp4reg", LOAD_WAIT) else {
        return failed("registration never settled", "the page reported nothing within 10 s");
    };
    let result = registration.get("result").cloned().unwrap_or_default();
    if let Err(error) = harness.enter_bundle("spike=sp4after") {
        return failed("reload of bundle A failed", format!("{error:#}"));
    }
    let controller = harness
        .wait_report("sp4ctl", LOAD_WAIT)
        .and_then(|report| report.get("controller").cloned())
        .unwrap_or_else(|| "no report".into());
    let worker_requests: Vec<(String, Option<String>, u16)> = harness
        .handler_requests()
        .into_iter()
        .filter(|record| record.uri.ends_with("/sw.js"))
        .map(|record| (record.method, record.service_worker, record.status))
        .collect();
    let handler_saw_script = worker_requests
        .iter()
        .any(|(_, header, status)| header.as_deref() == Some("script") && *status == 403);
    let rejected = result.starts_with("rejected");
    Outcome {
        observed: format!(
            "registration {result}; handler saw /sw.js {worker_requests:?}; controller after reload = {controller}"
        ),
        pass: handler_saw_script && rejected && controller == "null",
        notes: if result == "unavailable" {
            "navigator.serviceWorker is absent on this origin: registration is impossible, but the handler path was not exercised"
                .into()
        } else {
            "rerun with the filter forced to the pre-_22 call (protocol.md §12.5) by hand; this run uses the default".into()
        },
    }
}
