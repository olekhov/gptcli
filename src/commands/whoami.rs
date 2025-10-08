use anyhow::Result;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use crate::appconfig::load_effective;
use crate::fs as ufs;

pub async fn run() -> Result<()> {
    let root = ufs::detect_project_root()?;
    let eff = load_effective(&root)?;

    let client = reqwest::Client::builder().build()?;
    let url = format!("{}/models", eff.api_base.trim_end_matches('/'));
    let resp = client.get(url)
        .header(AUTHORIZATION, format!("Bearer {}", eff.api_key))
        .send().await?;

    println!("Profile:   {}", eff.profile_name);
    println!("API base:  {}", eff.api_base);
    println!("Model:     {}", eff.model);
    println!("Lang:      {}", eff.lang);
    println!("Status:    {}", resp.status());

    let hs: &HeaderMap = resp.headers();
    let get = |k:&str| hs.get(k).and_then(|v| v.to_str().ok()).unwrap_or("-");
    println!("Requests: remaining={} / limit={}, reset_in={}",
        get("x-ratelimit-remaining-requests"),
        get("x-ratelimit-limit-requests"),
        get("x-ratelimit-reset-requests"),
    );
    println!("Tokens:   remaining={} / limit={}, reset_in={}",
        get("x-ratelimit-remaining-tokens"),
        get("x-ratelimit-limit-tokens"),
        get("x-ratelimit-reset-tokens"),
    );
    // распечатать все x-ratelimit-* если есть
    for (k,v) in hs.iter() {
        if k.as_str().starts_with("x-ratelimit-") {
            if let Ok(s) = v.to_str() { println!("{:>28}: {}", k.as_str(), s); }
        }
    }
    Ok(())
}
