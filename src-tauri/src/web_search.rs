use crate::commands::SearchResult;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DdgResult {
    #[serde(default)]
    heading: String,
    #[serde(default)]
    first_url: String,
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DdgResponse {
    #[serde(default)]
    related_topics: Vec<DdgResult>,
    #[serde(default)]
    abstract_text: String,
    #[serde(default)]
    abstract_url: String,
    #[serde(default)]
    heading: String,
}

pub async fn search_ddg(query: &str) -> Result<Vec<SearchResult>, String> {
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        urlencoding::encode(query)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(&url)
        .header("User-Agent", "HarnAI/1.0")
        .send()
        .await
        .map_err(|e| format!("Search request failed: {}", e))?;

    let data: DdgResponse = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse search results: {}", e))?;

    let mut results = Vec::new();

    // Include instant answer if available
    if !data.abstract_text.is_empty() {
        results.push(SearchResult {
            title: data.heading,
            url: data.abstract_url,
            snippet: data.abstract_text,
        });
    }

    // Include related topics
    for topic in data.related_topics {
        if !topic.text.is_empty() && !topic.first_url.is_empty() {
            results.push(SearchResult {
                title: topic.heading,
                url: topic.first_url,
                snippet: topic.text,
            });
        }
        if results.len() >= 10 {
            break;
        }
    }

    Ok(results)
}
