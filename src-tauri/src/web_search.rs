use crate::commands::SearchResult;
use scraper::{Html, Selector};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

/// CSS selector for an individual result block on the DuckDuckGo HTML
/// results page. The page renders every organic result as a `div.result`
/// with `result__title`, `result__snippet`, etc. inside.
const RESULT_SELECTOR: &str = "div.result";

/// CSS selector for the title link inside a result. `h2.result__title > a.result__a`
/// is the canonical organic-result anchor.
const TITLE_LINK_SELECTOR: &str = "h2.result__title a.result__a";

/// CSS selector for the result snippet. DDG marks the snippet anchor with
/// `a.result__snippet` (it's an `<a>` because the whole row is clickable).
const SNIPPET_SELECTOR: &str = "a.result__snippet";

// ============================= Rate limiting =============================
//
// DDG's HTML endpoint is keyless but aggressively rate-limited. Naive
// concurrent calls — e.g. 16 subagents all firing `web_search` in parallel
// during a research burst — get throttled to a 202 Accepted with an
// empty body, which the parser surfaces as "No results returned". The
// fix is process-wide pacing on three axes:
//
//   1. A concurrency cap: at most N in-flight requests, no matter how
//      many callers are waiting. Excess callers block until a permit
//      frees up.
//   2. A minimum interval between the *start* of consecutive requests,
//      enforced by a held mutex. This serialises the rate of new
//      requests even when the concurrency cap would allow more.
//   3. Per-request retry with exponential backoff + jitter on
//      transient signals (202, 403, 408, 429, 5xx, network reset)
//      and on the "no results" parser outcome (since 202 looks
//      like a valid 2xx with a body that has zero `div.result`
//      blocks).
//
// The numbers below are conservative — they let a single agent make
// steady progress while still leaving headroom for the other 15
// subagents to interleave their searches without tripping DDG's
// throttler.

/// Max in-flight requests to DDG at any moment. 1 is conservative but
/// necessary: DDG throttles per-IP very aggressively, and even 2-3
/// concurrent requests can trip a soft-ban that takes 1-2 minutes to
/// clear. Single-flight keeps us reliably under the throttle ceiling.
const MAX_CONCURRENT_REQUESTS: usize = 1;

/// Minimum wall-clock gap between request starts. 4s with ±1s jitter
/// keeps us well under DDG's sustained-request rate. With single-flight
/// concurrency this is also the per-call floor, so 16 serial searches
/// take ~64s minimum — the cost of keyless web search.
const MIN_INTERVAL: Duration = Duration::from_millis(4_000);
const INTERVAL_JITTER: Duration = Duration::from_millis(1_000);

/// How many attempts a single `search_ddg` call will make before giving
/// up. With backoff (8s, 16s, 32s) worst-case wall time is roughly:
///   15s timeout + 8s + 16s + 32s + jitter ≈ 75s. Heavy, but the
/// alternative is the agent getting a useless "rate-limited" string
/// that it then has to retry from scratch anyway. Bump down if this
/// hurts interactive use; the retry budget is intentionally generous
/// because hitting it means the IP is already throttled and waiting
/// a minute is the only path to recovery.
const MAX_ATTEMPTS: u32 = 4;

/// Base for exponential backoff between retries. Doubles each attempt,
/// then we add uniform jitter on top.
const BACKOFF_BASE: Duration = Duration::from_millis(8_000);
const BACKOFF_JITTER: Duration = Duration::from_millis(3_000);

/// Process-wide concurrency cap. `OnceLock` so it's lazily initialised
/// on first call and shared by every `search_ddg` invocation in the
/// process — including the ones coming from 16 parallel subagent turns.
/// Returned as an `Arc` because `Semaphore::try_acquire_owned` takes
/// `self: Arc<Self>`.
fn concurrency_limiter() -> Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)))
        .clone()
}

/// Process-wide pacing lock. Holding the guard forces the next request
/// to wait for the previous one to "start" (we release immediately after
/// `Instant::now()` is recorded). This is intentionally simpler than a
/// real token bucket — we don't need burst handling, just a floor on
/// the inter-request gap.
fn pacing_lock() -> &'static Mutex<Instant> {
    static LOCK: OnceLock<Mutex<Instant>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(Instant::now() - MIN_INTERVAL))
}

// ============================= Public entry =============================

/// Search the web using DuckDuckGo's keyless HTML endpoint and return
/// the top organic results as `SearchResult` triples.
///
/// This used to call `api.duckduckgo.com` (the Instant Answer JSON API),
/// but that endpoint only returns results for Wikipedia-style knowledge
/// graph lookups and silently returns an empty `RelatedTopics` array for
/// almost any real query. The HTML search endpoint at
/// `html.duckduckgo.com/html/` is the same one DDG serves to non-JS
/// browsers and is what nearly every keyless search CLI uses as a
/// free fallback — it works for any query, no API key required.
///
/// Concurrency-safe: multiple parallel callers (e.g. 16 subagents) are
/// rate-limited and retried transparently. See the rate-limiting block
/// at the top of this file for the policy.
pub async fn search_ddg(query: &str) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Err("Search query is empty".to_string());
    }

    // Shared HTTP client. Building a new one per call would defeat the
    // connection pool and waste TLS handshakes.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) HarnAI/1.0")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let form: [(&str, &str); 7] = [
        ("q", query),
        ("kl", "us-en"),
        ("kp", "-2"),  // safe-search off
        ("kaf", "1"),  // auto-load more
        ("kac", "-1"), // auto-correct off
        ("kao", "-1"), // auto-redirect off
        ("kau", "1"),  // auto-suggest on
    ];

    let mut last_err: Option<String> = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            let backoff = backoff_with_jitter(attempt);
            tokio::time::sleep(backoff).await;
        }

        // Acquire a concurrency permit. Holding it for the duration of
        // the request means at most MAX_CONCURRENT_REQUESTS requests
        // are in flight at once, no matter how many callers are waiting.
        //
        // We use `try_acquire_owned` in a short-sleep loop rather than
        // the (newer) `acquire_owned` because the project pins tokio
        // 1.x and older 1.x releases only ship the `try_*` family. The
        // effect is identical for our purposes.
        let permit = loop {
            match concurrency_limiter().try_acquire_owned() {
                Ok(p) => break p,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        // Pace: ensure the configured minimum interval has passed since
        // the last request start. Releases the lock immediately after
        // recording the start time so the next caller can also start
        // waiting.
        {
            let mut last = pacing_lock().lock().await;
            let now = Instant::now();
            let earliest = *last + MIN_INTERVAL + random_jitter(INTERVAL_JITTER);
            if now < earliest {
                tokio::time::sleep(earliest - now).await;
            }
            *last = Instant::now();
            // `last` guard drops here, releasing the lock.
        }

        match fetch_and_parse(&client, &form).await {
            Ok(results) => {
                drop(permit);
                return Ok(results);
            }
            Err(SearchAttemptError::RateLimited) => {
                drop(permit);
                last_err = Some(format!(
                    "DuckDuckGo rate-limited the request (attempt {}/{})",
                    attempt + 1,
                    MAX_ATTEMPTS
                ));
                // Loop: backoff + retry.
            }
            Err(SearchAttemptError::Transport(msg)) => {
                drop(permit);
                last_err = Some(format!(
                    "Search transport error (attempt {}/{}): {}",
                    attempt + 1,
                    MAX_ATTEMPTS,
                    msg
                ));
                // Loop: backoff + retry. Network blips happen; a single
                // retry usually clears them.
            }
            Err(SearchAttemptError::Fatal(msg)) => {
                // Don't retry on programmer errors (bad URL, etc.) —
                // they won't get better with another attempt.
                drop(permit);
                return Err(msg);
            }
        }
    }

    // All attempts exhausted. Return the most recent error so the agent
    // can surface something useful.
    Err(last_err
        .unwrap_or_else(|| "Web search failed after multiple retries (rate limited?)".to_string()))
}

/// Errors an individual request attempt can produce. Used internally to
/// decide whether to retry.
enum SearchAttemptError {
    /// Got a response, but it was empty / 202 / parsed to zero results.
    /// Worth retrying — the next attempt will be paced further out.
    RateLimited,
    /// Network-level failure (timeout, connection reset, DNS, TLS). Worth
    /// retrying.
    Transport(String),
    /// Permanent failure (4xx other than 403/408/429, parse error on a
    /// non-empty body, etc.). Not worth retrying.
    Fatal(String),
}

impl std::fmt::Display for SearchAttemptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited => write!(f, "rate limited"),
            Self::Transport(s) => write!(f, "transport: {}", s),
            Self::Fatal(s) => write!(f, "fatal: {}", s),
        }
    }
}

/// One HTTP roundtrip + parse. Returns either the parsed results, a
/// rate-limit signal worth retrying, or a transport / fatal error.
async fn fetch_and_parse(
    client: &reqwest::Client,
    form: &[(&str, &str); 7],
) -> Result<Vec<SearchResult>, SearchAttemptError> {
    let resp = client
        .post("https://html.duckduckgo.com/html/")
        .form(form)
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
        .map_err(|e| SearchAttemptError::Transport(e.to_string()))?;

    let status = resp.status();

    // DDG's anti-bot returns a few different signals depending on how
    // badly the IP is throttled and how the request was made:
    //   * 202 Accepted with empty body — first-stage soft throttle.
    //   * 403 Forbidden — the IP has been hard-banned; will clear in
    //     minutes, definitely worth retrying with backoff.
    //   * 429 Too Many Requests — canonical rate-limit response.
    //   * 408 Request Timeout — server gave up before we did; retry.
    //   * 5xx — transient server hiccup, retry.
    let code = status.as_u16();
    let retryable_status =
        code == 202 || code == 403 || code == 408 || code == 429 || status.is_server_error();
    if retryable_status {
        return Err(SearchAttemptError::RateLimited);
    }
    if !status.is_success() {
        // 4xx other than 403/408/429 — bad request, not found, etc.
        // Permanent.
        return Err(SearchAttemptError::Fatal(format!(
            "Search backend returned HTTP {} {}",
            code,
            status.canonical_reason().unwrap_or("")
        )));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| SearchAttemptError::Transport(e.to_string()))?;

    match parse_results(&body, 8) {
        Ok(rs) => Ok(rs),
        // Parser says "no result blocks found" — for a non-trivial
        // query that's almost always a soft rate-limit or anti-bot
        // shim, not a genuine "no results" answer. Treat as retryable.
        Err(msg) if msg.contains("No results returned") => Err(SearchAttemptError::RateLimited),
        // Other parse errors are programmer-level (selector changed,
        // HTML malformed) — fail fast.
        Err(msg) => Err(SearchAttemptError::Fatal(msg)),
    }
}

// ============================= Parsing =============================

/// Parse the DuckDuckGo HTML results page and extract up to `max` organic
/// results. Exposed (rather than inlined into `search_ddg`) so it can be
/// unit-tested without a network round trip.
fn parse_results(html: &str, max: usize) -> Result<Vec<SearchResult>, String> {
    let document = Html::parse_document(html);

    let result_sel = Selector::parse(RESULT_SELECTOR)
        .map_err(|e| format!("Internal: bad result selector: {:?}", e))?;
    let title_sel = Selector::parse(TITLE_LINK_SELECTOR)
        .map_err(|e| format!("Internal: bad title selector: {:?}", e))?;
    let snippet_sel = Selector::parse(SNIPPET_SELECTOR)
        .map_err(|e| format!("Internal: bad snippet selector: {:?}", e))?;

    let mut results: Vec<SearchResult> = Vec::new();
    let mut seen_urls: std::collections::HashSet<String> = std::collections::HashSet::new();

    for block in document.select(&result_sel) {
        if results.len() >= max {
            break;
        }

        // Title: read the rendered text of the first matching anchor.
        // `text()` walks the entire subtree, so any inner `<b>` tags
        // DDG uses to highlight query terms are flattened to plain text.
        let title_a = match block.select(&title_sel).next() {
            Some(a) => a,
            None => continue,
        };
        let title = collapse_whitespace(&title_a.text().collect::<String>());
        if title.is_empty() {
            continue;
        }

        // URL: DDG wraps every result in a redirector at
        //   //duckduckgo.com/l/?uddg=<urlencoded real url>&rut=...
        // We want the real destination, not the redirector. `uddg` is
        // URL-encoded once; urlencoding::decode gives us the original.
        let raw_href = match title_a.value().attr("href") {
            Some(h) => h,
            None => continue,
        };
        let url = extract_real_url(raw_href);
        if url.is_empty() {
            continue;
        }
        if !seen_urls.insert(url.clone()) {
            // De-dupe — DDG occasionally surfaces the same page twice
            // (e.g. cached and live versions of the same URL).
            continue;
        }

        // Snippet: optional; some results (image packs, news) don't have one.
        let snippet = block
            .select(&snippet_sel)
            .next()
            .map(|s| collapse_whitespace(&s.text().collect::<String>()));

        results.push(SearchResult {
            title,
            url,
            snippet: snippet.unwrap_or_default(),
        });
    }

    if results.is_empty() {
        // Distinguish "parse succeeded but no results" from a transport
        // error. The retry loop above treats this as a soft rate-limit
        // signal worth another attempt.
        Err("No results returned by the search backend".to_string())
    } else {
        Ok(results)
    }
}

/// Pull the real destination URL out of a DuckDuckGo redirector link of
/// the form `//duckduckgo.com/l/?uddg=<urlencoded>&rut=...&...`.
///
/// Falls back to the input string when the link is already a plain
/// `http(s)://` URL (some result types — e.g. news, ads — aren't routed
/// through the redirector).
fn extract_real_url(href: &str) -> String {
    // Handle DDG's protocol-relative redirector: `//duckduckgo.com/l/...`
    let normalized = if href.starts_with("//") {
        format!("https:{}", href)
    } else if href.starts_with("/l/") {
        format!("https://duckduckgo.com{}", href)
    } else {
        href.to_string()
    };

    // Only attempt to extract `uddg` from duckduckgo.com redirect links.
    if !normalized.contains("duckduckgo.com/l/") {
        // Looks like a real, non-redirected URL. Pass it through as-is
        // so long as it's http(s); otherwise it's a junk link.
        if normalized.starts_with("http://") || normalized.starts_with("https://") {
            return normalized;
        }
        return String::new();
    }

    // Parse just the query string — we don't need the full URL crate here.
    let query_str = match normalized.split_once('?') {
        Some((_, q)) => q,
        None => return String::new(),
    };

    for pair in query_str.split('&') {
        if let Some(rest) = pair.strip_prefix("uddg=") {
            // The value can be percent-encoded once or twice in odd cases;
            // one decode is the common case, two covers the rest.
            let once = urlencoding::decode(rest)
                .map(|c| c.into_owned())
                .unwrap_or_default();
            if !once.is_empty() {
                return once;
            }
        }
    }

    String::new()
}

/// Collapse runs of whitespace (including HTML-line-break noise from
/// inline elements) into a single space and trim. Keeps titles/snippets
/// readable when they reach the LLM.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_was_space = true; // suppress leading whitespace
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_was_space {
                out.push(' ');
                prev_was_space = true;
            }
        } else {
            out.push(ch);
            prev_was_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

// ============================= Helpers =============================

/// Random jitter in `[0, max)`. Uses a simple xorshift PRNG seeded
/// from the thread-local clock — we don't need cryptographic quality,
/// just enough spread to avoid synchronised retry storms.
fn random_jitter(max: Duration) -> Duration {
    if max.is_zero() {
        return Duration::ZERO;
    }
    // nanos_in_max <= ~2.1B for any reasonable max, fits in u32.
    let nanos_in_max = max.as_nanos().min(u32::MAX as u128) as u32;
    if nanos_in_max == 0 {
        return Duration::ZERO;
    }
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // xorshift32 step
    let mut x = now_nanos ^ 0x9E37_79B9;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    Duration::from_nanos((x as u64) % (nanos_in_max as u64))
}

/// Backoff for retry attempt N (1-indexed). Doubles each attempt and
/// adds full jitter to spread concurrent retries.
fn backoff_with_jitter(attempt: u32) -> Duration {
    // attempt 1 → 1*BASE, attempt 2 → 2*BASE, attempt 3 → 4*BASE …
    let multiplier = 1u32 << (attempt - 1).min(8);
    let scaled = BACKOFF_BASE.saturating_mul(multiplier);
    scaled + random_jitter(BACKOFF_JITTER)
}

// ============================= Tests =============================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_real_url_decodes_uddg() {
        // The exact shape DDG emits.
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2F&rut=abc";
        assert_eq!(extract_real_url(href), "https://tokio.rs/");
    }

    #[test]
    fn extract_real_url_passes_through_plain_https() {
        let href = "https://example.com/path?q=1";
        assert_eq!(extract_real_url(href), "https://example.com/path?q=1");
    }

    #[test]
    fn extract_real_url_rejects_garbage() {
        assert_eq!(extract_real_url(""), "");
        assert_eq!(extract_real_url("javascript:void(0)"), "");
        assert_eq!(extract_real_url("mailto:x@y"), "");
    }

    #[test]
    fn extract_real_url_handles_relative_l_path() {
        let href = "/l/?uddg=https%3A%2F%2Fdocs.rs%2Ftokio";
        assert_eq!(extract_real_url(href), "https://docs.rs/tokio");
    }

    #[test]
    fn collapse_whitespace_basic() {
        assert_eq!(collapse_whitespace("  hello   world  "), "hello world");
        assert_eq!(collapse_whitespace("a\n\tb"), "a b");
        assert_eq!(collapse_whitespace(""), "");
    }

    #[test]
    fn parse_results_extracts_top_block() {
        // Minimal slice of the live HTML — just one result block.
        let html = r#"
            <html><body>
              <div class="serp__results"><div id="links">
                <div class="result results_links results_links_deep web-result ">
                  <div class="links_main links_deep result__body">
                    <h2 class="result__title">
                      <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2F&amp;rut=abc">Tokio - An asynchronous Rust runtime</a>
                    </h2>
                    <div class="result__extras"><div class="result__extras__url">
                      <a class="result__url" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2F&amp;rut=abc">tokio.rs</a>
                    </div></div>
                    <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2F&amp;rut=abc"><b>Tokio</b> is a runtime for writing reliable asynchronous applications with <b>Rust</b>.</a>
                  </div>
                </div>
              </div></div>
            </body></html>
        "#;
        let r = parse_results(html, 8).expect("should parse");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "Tokio - An asynchronous Rust runtime");
        assert_eq!(r[0].url, "https://tokio.rs/");
        assert!(r[0].snippet.contains("runtime"));
        assert!(!r[0].snippet.contains("<b>"));
    }

    #[test]
    fn parse_results_dedupes() {
        let html = r#"
            <html><body>
              <div class="result">
                <h2 class="result__title"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&amp;rut=1">First</a></h2>
                <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&amp;rut=1">snippet one</a>
              </div>
              <div class="result">
                <h2 class="result__title"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&amp;rut=2">First again</a></h2>
                <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&amp;rut=2">snippet two</a>
              </div>
            </body></html>
        "#;
        let r = parse_results(html, 8).expect("should parse");
        assert_eq!(r.len(), 1, "duplicate URLs should collapse");
        assert_eq!(r[0].url, "https://example.com/");
    }

    #[test]
    fn parse_results_empty_html_signals_rate_limit() {
        // Empty / no-result responses are retryable signals at the call
        // site — `search_ddg` maps them to `RateLimited`. The parser
        // itself just returns the error string; the retry decision lives
        // in `fetch_and_parse`.
        match parse_results("<html><body>no results here</body></html>", 8) {
            Err(err) => assert!(err.contains("No results returned")),
            Ok(_) => panic!("empty HTML should yield an error"),
        }
    }

    #[test]
    fn random_jitter_in_range() {
        for _ in 0..100 {
            let j = random_jitter(Duration::from_millis(500));
            assert!(j < Duration::from_millis(500));
        }
    }

    #[test]
    fn random_jitter_zero_when_max_zero() {
        assert_eq!(random_jitter(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn backoff_grows_with_attempt() {
        // We can't assert exact values (jitter is random) but the
        // minimum should grow at least linearly with attempt.
        let b1 = backoff_with_jitter(1);
        let b2 = backoff_with_jitter(2);
        let b3 = backoff_with_jitter(3);
        // Each backoff is at least BASE (jitter is additive, not
        // subtractive), so the min should double between attempts.
        assert!(b1 >= BACKOFF_BASE);
        assert!(b2 >= BACKOFF_BASE.saturating_mul(2));
        assert!(b3 >= BACKOFF_BASE.saturating_mul(4));
    }
}
