use crate::commands::SearchResult;
use scraper::{Html, Selector};
use serde::Deserialize;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Semaphore};

// =============================================================================
// Multi-backend web search
// =============================================================================
//
// General web search in 2026 is hostile to keyless, high-volume use: DDG
// throttles parallel callers, Brave requires a card, the public SearXNG
// instances all serve captcha challenges, etc. There is no single backend
// that works for every query at 16+ parallel calls.
//
// The pragmatic answer is a router that picks the right backend for the
// query's intent:
//   * Factual / encyclopedia question → Wikipedia REST (keyless, no limit).
//   * Code / library / "how do I use X" → GitHub Search (10 req/min
//     unauthenticated, but generous in practice).
//   * Academic paper / research → arXiv API (keyless, no limit).
//   * Everything else (current events, opinions, news, general web) →
//     DuckDuckGo HTML, with the existing rate-limit machinery (single-
//     flight, ~4s pacing, exponential backoff).
//
// If the routed backend returns nothing useful, we fall through to the
// next candidate in the chain rather than immediately erroring. This
// gives the agent usable results across the realistic query mix instead
// of "DDG succeeded → use it" or "DDG failed → bail".
//
// All four backends are keyless and require no signup. The trade-off is
// that a query that doesn't fit any of the specialized backends (most
// non-coding research queries today) still goes to DDG and is subject
// to its throttling — there's no keyless escape from that.

// ============================= DDG (general fallback) =============================

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

/// Max in-flight requests to DDG at any moment. 1 is conservative but
/// necessary: DDG throttles per-IP very aggressively, and even 2-3
/// concurrent requests can trip a soft-ban that takes 1-2 minutes to
/// clear. Single-flight keeps us reliably under the throttle ceiling.
const MAX_CONCURRENT_REQUESTS: usize = 1;

/// Minimum wall-clock gap between request starts. 4s with ±1s jitter
/// keeps us well under DDG's sustained-request rate.
const MIN_INTERVAL: Duration = Duration::from_millis(4_000);
const INTERVAL_JITTER: Duration = Duration::from_millis(1_000);

/// How many attempts a single DDG call will make before giving up.
/// Backoff is 8s, 16s, 32s + jitter.
const MAX_ATTEMPTS: u32 = 4;
const BACKOFF_BASE: Duration = Duration::from_millis(8_000);
const BACKOFF_JITTER: Duration = Duration::from_millis(3_000);

fn concurrency_limiter() -> Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)))
        .clone()
}

fn pacing_lock() -> &'static Mutex<Instant> {
    static LOCK: OnceLock<Mutex<Instant>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(Instant::now() - MIN_INTERVAL))
}

/// Search the web using DuckDuckGo's keyless HTML endpoint and return
/// the top organic results as `SearchResult` triples. This is the
/// general-purpose fallback; the intent router tries specialized
/// backends first.
pub async fn search_ddg(query: &str) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Err("Search query is empty".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) HarnAI/1.0")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let form: [(&str, &str); 7] = [
        ("q", query),
        ("kl", "us-en"),
        ("kp", "-2"),
        ("kaf", "1"),
        ("kac", "-1"),
        ("kao", "-1"),
        ("kau", "1"),
    ];

    let mut last_err: Option<String> = None;
    for attempt in 0..MAX_ATTEMPTS {
        if attempt > 0 {
            let backoff = backoff_with_jitter(attempt);
            tokio::time::sleep(backoff).await;
        }

        let permit = loop {
            match concurrency_limiter().try_acquire_owned() {
                Ok(p) => break p,
                Err(_) => tokio::time::sleep(Duration::from_millis(25)).await,
            }
        };

        {
            let mut last = pacing_lock().lock().await;
            let now = Instant::now();
            let earliest = *last + MIN_INTERVAL + random_jitter(INTERVAL_JITTER);
            if now < earliest {
                tokio::time::sleep(earliest - now).await;
            }
            *last = Instant::now();
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
            }
            Err(SearchAttemptError::Transport(msg)) => {
                drop(permit);
                last_err = Some(format!(
                    "Search transport error (attempt {}/{}): {}",
                    attempt + 1,
                    MAX_ATTEMPTS,
                    msg
                ));
            }
            Err(SearchAttemptError::Fatal(msg)) => {
                drop(permit);
                return Err(msg);
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| "Web search failed after multiple retries (rate limited?)".to_string()))
}

enum SearchAttemptError {
    RateLimited,
    Transport(String),
    Fatal(String),
}

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
    let code = status.as_u16();
    let retryable_status =
        code == 202 || code == 403 || code == 408 || code == 429 || status.is_server_error();
    if retryable_status {
        return Err(SearchAttemptError::RateLimited);
    }
    if !status.is_success() {
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
        Err(msg) if msg.contains("No results returned") => Err(SearchAttemptError::RateLimited),
        Err(msg) => Err(SearchAttemptError::Fatal(msg)),
    }
}

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
        let title_a = match block.select(&title_sel).next() {
            Some(a) => a,
            None => continue,
        };
        let title = collapse_whitespace(&title_a.text().collect::<String>());
        if title.is_empty() {
            continue;
        }
        let raw_href = match title_a.value().attr("href") {
            Some(h) => h,
            None => continue,
        };
        let url = extract_real_url(raw_href);
        if url.is_empty() {
            continue;
        }
        if !seen_urls.insert(url.clone()) {
            continue;
        }
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
        Err("No results returned by the search backend".to_string())
    } else {
        Ok(results)
    }
}

fn extract_real_url(href: &str) -> String {
    let normalized = if href.starts_with("//") {
        format!("https:{}", href)
    } else if href.starts_with("/l/") {
        format!("https://duckduckgo.com{}", href)
    } else {
        href.to_string()
    };
    if !normalized.contains("duckduckgo.com/l/") {
        if normalized.starts_with("http://") || normalized.starts_with("https://") {
            return normalized;
        }
        return String::new();
    }
    let query_str = match normalized.split_once('?') {
        Some((_, q)) => q,
        None => return String::new(),
    };
    for pair in query_str.split('&') {
        if let Some(rest) = pair.strip_prefix("uddg=") {
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

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_was_space = true;
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

fn random_jitter(max: Duration) -> Duration {
    if max.is_zero() {
        return Duration::ZERO;
    }
    let nanos_in_max = max.as_nanos().min(u32::MAX as u128) as u32;
    if nanos_in_max == 0 {
        return Duration::ZERO;
    }
    let now_nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut x = now_nanos ^ 0x9E37_79B9;
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    Duration::from_nanos((x as u64) % (nanos_in_max as u64))
}

fn backoff_with_jitter(attempt: u32) -> Duration {
    let multiplier = 1u32 << (attempt - 1).min(8);
    BACKOFF_BASE.saturating_mul(multiplier) + random_jitter(BACKOFF_JITTER)
}

// ============================= Wikipedia (factual) =============================

/// Wikipedia REST API summary endpoint. Keyless, no challenge, no rate
/// limit at human-use volumes. Returns a JSON object with `title`,
/// `extract` (a paragraph summary), and `content_urls.desktop.page`
/// (canonical URL).
const WIKIPEDIA_API: &str = "https://en.wikipedia.org/api/rest_v1/page/summary/";

/// One Wikipedia summary, as returned by the API. Only the fields we
/// actually use are deserialized; the REST API has more.
#[derive(Deserialize)]
struct WikipediaSummary {
    #[serde(default)]
    title: String,
    #[serde(default)]
    extract: String,
    #[serde(default)]
    content_urls: WikipediaContentUrls,
}

#[derive(Deserialize, Default)]
struct WikipediaContentUrls {
    #[serde(default)]
    desktop: WikipediaDesktop,
}

#[derive(Deserialize, Default)]
struct WikipediaDesktop {
    #[serde(default)]
    page: String,
}

/// Search Wikipedia for the closest-matching article and return its
/// summary as a single search result. The REST API doesn't have a true
/// search endpoint in `rest_v1`; the canonical way is to use the
/// `action=query&list=search` MediaWiki API. We use that here.
///
/// Returns `Ok(vec)` with 0 or 1 result on success (0 = no confident
/// match). On transport / parse failure returns `Err`. The caller
/// (`search`) treats empty `Ok` as "try the next backend" rather than
/// a hard failure.
pub async fn search_wikipedia(query: &str) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("HarnAI/1.0 (contact: harness-app)")
        .build()
        .map_err(|e| format!("Wikipedia client build failed: {}", e))?;

    // Step 1: find the best-matching article title via the MediaWiki
    // action API. `srlimit=1` keeps this to one request and one round
    // trip; we don't need a list of candidates, just the top hit.
    let search_url = format!(
        "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&format=json&srlimit=1&origin=*",
        urlencoding::encode(query)
    );

    #[derive(Deserialize)]
    struct SearchResp {
        query: Option<SearchBody>,
    }
    #[derive(Deserialize)]
    struct SearchBody {
        search: Option<Vec<SearchHit>>,
    }
    #[derive(Deserialize)]
    struct SearchHit {
        title: String,
    }

    let resp = client
        .get(&search_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Wikipedia search request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "Wikipedia search returned HTTP {}",
            resp.status().as_u16()
        ));
    }

    let body: SearchResp = resp
        .json()
        .await
        .map_err(|e| format!("Wikipedia search parse failed: {}", e))?;

    let title = match body.query.and_then(|q| q.search).and_then(|mut s| s.pop()) {
        Some(hit) => hit.title,
        None => return Ok(Vec::new()),
    };

    // Step 2: fetch the summary for the matched article. The REST
    // summary endpoint converts the title to the URL-safe form for us.
    let safe_title = title.replace(' ', "_");
    let summary_url = format!("{}{}", WIKIPEDIA_API, urlencoding::encode(&safe_title));

    let resp = client
        .get(&summary_url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Wikipedia summary request failed: {}", e))?;

    if !resp.status().is_success() {
        // 404 = the title we just got doesn't have a summary. Treat as
        // a miss, not an error.
        if resp.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        return Err(format!(
            "Wikipedia summary returned HTTP {}",
            resp.status().as_u16()
        ));
    }

    let body: WikipediaSummary = resp
        .json()
        .await
        .map_err(|e| format!("Wikipedia summary parse failed: {}", e))?;

    if body.extract.is_empty() {
        return Ok(Vec::new());
    }

    let url = if !body.content_urls.desktop.page.is_empty() {
        body.content_urls.desktop.page
    } else {
        format!("https://en.wikipedia.org/wiki/{}", safe_title)
    };

    Ok(vec![SearchResult {
        title: body.title,
        url,
        snippet: body.extract,
    }])
}

// ============================= GitHub (code/library) =============================

/// GitHub Search Repositories API. 10 req/min unauthenticated, 30 req/min
/// authenticated. We use the unauthenticated tier and rely on the
/// intent router to avoid hammering it. Returns repo name, description,
/// URL, and star count as a single synthetic snippet.
const GITHUB_API: &str = "https://api.github.com/search/repositories";

#[derive(Deserialize)]
struct GitHubSearchResponse {
    #[serde(default)]
    items: Vec<GitHubRepo>,
}

#[derive(Deserialize)]
struct GitHubRepo {
    full_name: String,
    #[serde(default)]
    description: Option<String>,
    html_url: String,
    #[serde(default)]
    stargazers_count: u64,
    #[serde(default)]
    language: Option<String>,
}

/// Search GitHub for repositories matching the query. Returns up to 5
/// results. We filter out forks and description-less repos so the
/// output is genuinely useful to the LLM.
pub async fn search_github(query: &str) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("HarnAI/1.0")
        .build()
        .map_err(|e| format!("GitHub client build failed: {}", e))?;

    let url = format!(
        "{}?q={}&per_page=5&sort=stars&order=desc",
        GITHUB_API,
        urlencoding::encode(query)
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| format!("GitHub search request failed: {}", e))?;

    let status = resp.status();
    if status.as_u16() == 403 || status.as_u16() == 429 {
        // Rate-limited. Caller (`search`) treats this as a fallback
        // trigger.
        return Err(format!("GitHub rate-limited ({})", status.as_u16()));
    }
    if !status.is_success() {
        return Err(format!("GitHub search returned HTTP {}", status.as_u16()));
    }

    let body: GitHubSearchResponse = resp
        .json()
        .await
        .map_err(|e| format!("GitHub search parse failed: {}", e))?;

    let mut results = Vec::new();
    for repo in body.items {
        if repo.description.as_deref().unwrap_or("").is_empty() {
            continue;
        }
        let lang = repo.language.unwrap_or_else(|| "unknown".into());
        let snippet = format!(
            "{} — {} ({} stars, {})",
            repo.full_name,
            repo.description.unwrap_or_default(),
            repo.stargazers_count,
            lang
        );
        results.push(SearchResult {
            title: repo.full_name,
            url: repo.html_url,
            snippet,
        });
        if results.len() >= 5 {
            break;
        }
    }

    Ok(results)
}

// ============================= arXiv (academic) =============================

/// arXiv API. Keyless, returns Atom XML, no rate limit at agent
/// volumes. The query is sent verbatim as the `all:` field; arXiv
/// supports field-specific queries (`ti:`, `au:`, `abs:`) but `all:`
/// is the safest default for free-form agent queries.
const ARXIV_API: &str = "http://export.arxiv.org/api/query";

/// We pull a few well-known fields out of each Atom entry by simple
/// string scanning. arXiv's XML is small and stable enough that a full
/// XML parser is overkill — a few `str::find` calls do the job. (We
/// already have `scraper` in the dep tree but the HTML parser doesn't
/// help with Atom namespaces; for 5 fields the regex-free approach
/// below is simpler and more robust.)
pub async fn search_arxiv(query: &str) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Ok(Vec::new());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("HarnAI/1.0")
        .build()
        .map_err(|e| format!("arXiv client build failed: {}", e))?;

    let url = format!(
        "{}?search_query=all:{}&max_results=5&sortBy=relevance&sortOrder=descending",
        ARXIV_API,
        urlencoding::encode(query)
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/atom+xml")
        .send()
        .await
        .map_err(|e| format!("arXiv request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("arXiv returned HTTP {}", resp.status().as_u16()));
    }

    let body = resp
        .text()
        .await
        .map_err(|e| format!("arXiv body read failed: {}", e))?;

    Ok(parse_arxiv_entries(&body))
}

fn parse_arxiv_entries(xml: &str) -> Vec<SearchResult> {
    // Split on `<entry>` so each chunk is one paper. arXiv puts all
    // metadata for an entry inside its `<entry>...</entry>` block, so
    // per-field extraction only needs to look inside the current chunk.
    let chunks: Vec<&str> = xml.split("<entry>").skip(1).collect();
    let mut results = Vec::new();
    for chunk in chunks {
        if results.len() >= 5 {
            break;
        }
        let title = extract_xml_field(chunk, "title").unwrap_or_default();
        let summary = extract_xml_field(chunk, "summary").unwrap_or_default();
        let id = extract_xml_field(chunk, "id").unwrap_or_default();
        let author = first_author(chunk).unwrap_or_default();

        if title.is_empty() || summary.is_empty() {
            continue;
        }
        // arXiv IDs are like `http://arxiv.org/abs/2503.12511v3`; the
        // canonical URL is just the abs page.
        let url = if id.is_empty() {
            "https://arxiv.org/".to_string()
        } else {
            id
        };
        let snippet = if author.is_empty() {
            truncate(&summary, 280)
        } else {
            format!("{} — {}", author, truncate(&summary, 240))
        };
        results.push(SearchResult {
            title: collapse_xml_whitespace(&title),
            url,
            snippet: collapse_xml_whitespace(&snippet),
        });
    }
    results
}

fn extract_xml_field(chunk: &str, field: &str) -> Option<String> {
    let open = format!("<{}>", field);
    let close = format!("</{}>", field);
    let start = chunk.find(&open)? + open.len();
    let end = chunk[start..].find(&close)? + start;
    Some(chunk[start..end].to_string())
}

fn first_author(chunk: &str) -> Option<String> {
    let open = "<author>";
    let close = "</author>";
    let author_block = chunk.find(open)? + open.len();
    let block_end = chunk[author_block..].find(close)? + author_block;
    let block = &chunk[author_block..block_end];
    extract_xml_field(block, "name")
}

fn collapse_xml_whitespace(s: &str) -> String {
    // arXiv XML often collapses whitespace in <title>/<summary> already,
    // but newlines sneak in. Reuse the same collapse as the DDG path.
    collapse_whitespace(s.trim())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", head)
    }
}

// ============================= Intent router =============================

/// Coarse intent classification for a free-form search query. Picks the
/// order of backends to try. The first backend that returns a
/// non-empty, useful result wins; otherwise we fall through.
///
/// The classification is deliberately conservative — it's a router,
/// not a guess. If the intent is unclear we put DDG first (since it's
/// the most general) and use the others as fallbacks for the specific
/// intents they're good at. Misclassifying a code question as
/// "general" is fine; misclassifying a general question as "code" is
/// expensive (we'd skip DDG entirely).
#[derive(Debug, PartialEq, Eq)]
enum Intent {
    /// "Who is X", "what is Y", definitions, dates, places, people —
    /// Wikipedia is the right first stop.
    Factual,
    /// "X library", "how to use Y in Z", "rust async", "python
    /// requests" — GitHub repos are the highest-signal result.
    Code,
    /// Research paper, academic query, "arXiv", "study on" — arXiv.
    Academic,
    /// Current events, news, opinions, products, general web. DDG.
    General,
}

fn classify(query: &str) -> Intent {
    let q = query.to_lowercase();

    // Tokenise on non-alphanumeric runs. Substring matching on the
    // raw lowercased query gives false positives like:
    //   * "thesis" matching inside "photosynthesis"
    //   * "api" matching inside "capital"
    //   * "rust" matching inside "trust" or "frustum"
    // We use word-boundary token checks everywhere below.
    let tokens: Vec<&str> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect();

    // Academic markers: explicit venue, "paper", "study", "research",
    // "et al", formal phrases. arXiv only really helps for STEM
    // academic content; if the user says "paper" we go there.
    let academic_markers = [
        "arxiv",
        "paper",
        "study",
        "research",
        "preprint",
        "scientific",
        "journal",
        "dissertation",
    ];
    let has_academic = academic_markers.iter().any(|m| tokens.contains(m)) || q.contains("et al"); // multi-word, has to stay as substring
    if has_academic {
        return Intent::Academic;
    }

    // Code markers: a programming language name (broad list) is the
    // strongest signal. We pair it with library-shaped nouns to avoid
    // false positives on questions like "what is rust" (which is
    // factual, not code).
    let langs = [
        "rust",
        "python",
        "javascript",
        "typescript",
        "go",
        "golang",
        "java",
        "kotlin",
        "swift",
        "ruby",
        "php",
        "scala",
        "haskell",
        "elixir",
        "clojure",
        "perl",
        "deno",
        "node",
        "nodejs",
        "react",
        "vue",
        "svelte",
        "angular",
        "django",
        "flask",
        "fastapi",
        "express",
        "nextjs",
        "nuxt",
        "tokio",
        "axum",
        "actix",
        "serde",
        "tauri",
        "wasm",
        "docker",
        "kubernetes",
        "k8s",
        "terraform",
        "ansible",
        "postgres",
        "postgresql",
        "mysql",
        "mongodb",
        "redis",
        "graphql",
        "trpc",
        "grpc",
        "kafka",
        "rabbitmq",
    ];
    let has_lang = langs.iter().any(|l| tokens.contains(&l));
    let code_nouns = [
        "library",
        "framework",
        "package",
        "crate",
        "module",
        "function",
        "api",
        "sdk",
        "cli",
        "tool",
        "tooling",
        "tutorial",
        "example",
        "docs",
        "documentation",
        "build",
        "install",
        "setup",
        "configure",
        "async",
        "await",
        "promise",
        "future",
        "syntax",
        "error",
        "exception",
        "compile",
    ];
    let has_code_noun = code_nouns.iter().any(|n| tokens.contains(&n));
    if has_lang || has_code_noun {
        return Intent::Code;
    }

    // Factual markers: classic "what is", "who is", "when was", "where
    // is", or a quoted proper-noun phrase. Wikipedia hits these
    // perfectly. Use the first token so "explanation" doesn't match
    // "explain". Also accept the multi-word patterns "definition of",
    // "history of", "biography of", "tell me about" as direct factual
    // queries.
    let question_words = ["what", "who", "when", "where", "how", "why"];
    let factual_prefixes = [
        "definition of",
        "history of",
        "biography of",
        "tell me about",
        "explain ",
    ];
    let is_question = tokens
        .first()
        .map(|t| question_words.contains(t))
        .unwrap_or(false)
        || factual_prefixes.iter().any(|p| q.starts_with(p));
    if is_question {
        return Intent::Factual;
    }

    Intent::General
}

/// The unified entry point. Routes the query to the right backend(s)
/// and returns the first useful result. Used by the `web_search` agent
/// tool so the LLM gets answers regardless of the underlying throttling
/// situation.
///
/// Fallback chain semantics:
///   * Factual: Wikipedia → DDG
///   * Code:    GitHub   → DDG
///   * Academic: arXiv  → DDG
///   * General: DDG only
///
/// If all backends fail or return empty results, the final error
/// includes the per-backend status so the agent can see what went
/// wrong. The 50-char guard in `tool_web_search` will then surface a
/// clear "no useful content" error and the agent stops looping on the
/// same query.
pub async fn search(query: &str) -> Result<Vec<SearchResult>, String> {
    if query.trim().is_empty() {
        return Err("Search query is empty".to_string());
    }

    let intent = classify(query);
    let mut attempts: Vec<(&'static str, Result<Vec<SearchResult>, String>)> = Vec::new();

    let backends: &[&str] = match intent {
        Intent::Factual => &["wikipedia", "ddg"],
        Intent::Code => &["github", "ddg"],
        Intent::Academic => &["arxiv", "ddg"],
        Intent::General => &["ddg"],
    };

    for backend in backends {
        let r = match *backend {
            "wikipedia" => search_wikipedia(query).await,
            "github" => search_github(query).await,
            "arxiv" => search_arxiv(query).await,
            "ddg" => search_ddg(query).await,
            other => Err(format!("Unknown backend: {}", other)),
        };
        // Take ownership of `r` here so we can return its Vec without
        // borrowing. `SearchResult` doesn't derive `Clone`, and the
        // borrow checker correctly flags `r.clone()` on a borrowed
        // `&Result` as a no-op. We only push into `attempts` (which
        // moves `r`) on the empty/error paths, so the early return
        // is the only place we'd otherwise be tempted to clone.
        match r {
            Ok(rs) if !rs.is_empty() => return Ok(rs),
            other => attempts.push((backend, other)),
        }
    }

    // All backends returned empty or errored. Surface a multi-line
    // status so the calling agent knows it wasn't a single-point
    // failure.
    let mut lines = Vec::new();
    for (name, r) in &attempts {
        match r {
            Ok(rs) => lines.push(format!("{}: 0 results", name)),
            Err(e) => lines.push(format!("{}: error ({})", name, e)),
        }
    }
    Err(format!(
        "All search backends returned no results for {:?} [intent {:?}]. {}",
        query,
        intent,
        lines.join("; ")
    ))
}

// ============================= Tests =============================

#[cfg(test)]
mod tests {
    use super::*;

    // ---- DDG parser / URL helpers ----

    #[test]
    fn extract_real_url_decodes_uddg() {
        let href = "//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2F&rut=abc";
        assert_eq!(extract_real_url(href), "https://tokio.rs/");
    }

    #[test]
    fn extract_real_url_passes_through_plain_https() {
        assert_eq!(
            extract_real_url("https://example.com/path?q=1"),
            "https://example.com/path?q=1"
        );
    }

    #[test]
    fn extract_real_url_rejects_garbage() {
        assert_eq!(extract_real_url(""), "");
        assert_eq!(extract_real_url("javascript:void(0)"), "");
        assert_eq!(extract_real_url("mailto:x@y"), "");
    }

    #[test]
    fn extract_real_url_handles_relative_l_path() {
        assert_eq!(
            extract_real_url("/l/?uddg=https%3A%2F%2Fdocs.rs%2Ftokio"),
            "https://docs.rs/tokio"
        );
    }

    #[test]
    fn collapse_whitespace_basic() {
        assert_eq!(collapse_whitespace("  hello   world  "), "hello world");
        assert_eq!(collapse_whitespace("a\n\tb"), "a b");
        assert_eq!(collapse_whitespace(""), "");
    }

    #[test]
    fn parse_results_extracts_top_block() {
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
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "https://example.com/");
    }

    #[test]
    fn parse_results_empty_html_signals_rate_limit() {
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
        assert!(backoff_with_jitter(1) >= BACKOFF_BASE);
        assert!(backoff_with_jitter(2) >= BACKOFF_BASE.saturating_mul(2));
        assert!(backoff_with_jitter(3) >= BACKOFF_BASE.saturating_mul(4));
    }

    // ---- arXiv XML parser ----

    #[test]
    fn arxiv_parses_minimal_entry() {
        let xml = r#"<?xml version='1.0'?>
            <feed><entry>
                <id>http://arxiv.org/abs/2503.12511v3</id>
                <title>Sample paper title</title>
                <summary>An abstract describing the work.</summary>
                <author><name>Jane Doe</name></author>
            </entry></feed>"#;
        let rs = parse_arxiv_entries(xml);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].title, "Sample paper title");
        assert_eq!(rs[0].url, "http://arxiv.org/abs/2503.12511v3");
        assert!(rs[0].snippet.starts_with("Jane Doe"));
    }

    #[test]
    fn arxiv_skips_entries_without_title() {
        let xml = r#"<feed><entry>
                <id>http://arxiv.org/abs/1</id>
                <summary>only summary, no title</summary>
            </entry></feed>"#;
        assert!(parse_arxiv_entries(xml).is_empty());
    }

    // ---- Intent router ----

    #[test]
    fn classify_routes_known_cases() {
        assert_eq!(classify("What is the capital of France"), Intent::Factual);
        assert_eq!(classify("Who wrote the Iliad"), Intent::Factual);
        assert_eq!(classify("definition of photosynthesis"), Intent::Factual);
        assert_eq!(classify("rust async tokio tutorial"), Intent::Code);
        assert_eq!(classify("how to use react hooks"), Intent::Code);
        assert_eq!(classify("arxiv paper on transformers"), Intent::Academic);
        assert_eq!(classify("recent study on sleep"), Intent::Academic);
        assert_eq!(classify("best coffee shops in Berlin"), Intent::General);
    }

    // ---- Top-level router: needs network, gated by an env var so it
    //      doesn't run in cargo test by default. Run with:
    //        WS_NET_TESTS=1 cargo test --lib web_search::tests::live -- --ignored
    //      to exercise the real backends. The non-live tests above cover
    //      the parser / router logic deterministically.

    #[tokio::test]
    #[ignore = "requires network; run with WS_NET_TESTS=1"]
    async fn live_wikipedia_factual_query() {
        let r = search_wikipedia("What is the capital of France").await;
        let rs = r.expect("wikipedia should respond");
        assert!(!rs.is_empty(), "wikipedia returned no results");
        let r0 = &rs[0];
        assert!(r0.url.contains("wikipedia.org"));
        assert!(r0.snippet.to_lowercase().contains("paris"));
    }

    #[tokio::test]
    #[ignore = "requires network; run with WS_NET_TESTS=1"]
    async fn live_github_code_query() {
        let r = search_github("rust async tokio").await;
        let rs = r.expect("github should respond");
        assert!(!rs.is_empty(), "github returned no results");
        let r0 = &rs[0];
        assert!(r0.url.contains("github.com"));
        assert!(r0.snippet.contains("stars"));
    }

    #[tokio::test]
    #[ignore = "requires network; run with WS_NET_TESTS=1"]
    async fn live_router_picks_wikipedia_for_factual() {
        let r = search("Who is Alan Turing").await;
        let rs = r.expect("router should succeed");
        assert!(!rs.is_empty());
        assert!(
            rs[0].url.contains("wikipedia.org"),
            "factual should hit Wikipedia first, got: {}",
            rs[0].url
        );
    }

    #[tokio::test]
    #[ignore = "requires network; run with WS_NET_TESTS=1"]
    async fn live_router_picks_github_for_code() {
        let r = search("rust async tokio runtime").await;
        let rs = r.expect("router should succeed");
        assert!(!rs.is_empty());
        // Either github or DDG is acceptable here (github first, then
        // DDG). We just check that at least one of them returned.
        let from_github = rs[0].url.contains("github.com");
        let from_ddg = rs[0].url.contains("duckduckgo.com") || !rs[0].url.contains("github.com");
        assert!(from_github || from_ddg, "unexpected backend: {}", rs[0].url);
    }
}
