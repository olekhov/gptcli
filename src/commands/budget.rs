
use anyhow::Result;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use std::env;

pub async fn run() -> Result<()> {
    let key = env::var("OPENAI_API_KEY")?;
    let url = "https://api.openai.com/v1/models"; // лёгкий эндпоинт
    let client = reqwest::Client::builder().build()?;

    let resp = client.get(url)
        .header(AUTHORIZATION, format!("Bearer {}", key))
        .send().await?;

    let hs: &HeaderMap = resp.headers();

    // полезные заголовки, если сервер их отдал
    let req_rem = hs.get("x-ratelimit-remaining-requests").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let req_limit = hs.get("x-ratelimit-limit-requests").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let req_reset = hs.get("x-ratelimit-reset-requests").and_then(|v| v.to_str().ok()).unwrap_or("-");

    let tok_rem = hs.get("x-ratelimit-remaining-tokens").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let tok_limit = hs.get("x-ratelimit-limit-tokens").and_then(|v| v.to_str().ok()).unwrap_or("-");
    let tok_reset = hs.get("x-ratelimit-reset-tokens").and_then(|v| v.to_str().ok()).unwrap_or("-");

    println!("Requests: remaining={req_rem} / limit={req_limit}, reset_in={req_reset}");
    println!("Tokens:   remaining={tok_rem} / limit={tok_limit}, reset_in={tok_reset}");

    // на некоторых тарифах ещё бывают minute/day/burst-линейки; можно вывести все x-ratelimit-* для наглядности:
    for (k,v) in hs.iter() {
        if k.as_str().starts_with("x-ratelimit-") {
            if let Ok(s) = v.to_str() { println!("{:>28}: {}", k.as_str(), s); }
        }
    }

    Ok(())
}
