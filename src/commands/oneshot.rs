use anyhow::{bail, Context, Result};
use async_openai::{types::responses::{ContentType, CreateResponseArgs, Input, InputContent, InputFileArgs, InputItem, InputMessageArgs, InputMessageType, InputText, Role, Usage}, Client};
use time::OffsetDateTime;
use std::{fs, path::{Path, PathBuf}};

use crate::{appconfig::load_effective, commands::extract_output_text, context::AppCtx, fs as ufs};

pub async fn run(
    ctx: &AppCtx,
    system: &Option<String>,
    user: &str,
    file: &Option<PathBuf>,
) -> Result<()> {
    let root = &ctx.root;
    let cfg = &ctx.eff;

    let system : String = system.clone().unwrap_or("".to_string());

    let (text, usage, req_path, resp_path) = call_openai(&cfg.model, cfg.max_output_tokens, user, &system, file).await?;

    println!("{text}\n");
    println!("Usage: {:?}", usage);
    eprintln!("— raw request:  {req_path}");
    eprintln!("— raw response: {resp_path}");
    Ok(())
}



async fn call_openai(model: &str, max_output:u32, user:&str, system:&str, file: &Option<PathBuf>)
-> Result<(String, Option<Usage>, String, String)> {
    // messages → Input

    let system_msg = InputItem::Message(
        InputMessageArgs::default()
            .kind(InputMessageType::Message)                // можно опустить: Default
            .role(Role::System)
            .content(InputContent::TextInput(system.to_string())) // <-- оборачиваем текст
            .build()?
    );

    let user_msg = InputItem::Message(
        InputMessageArgs::default()
            .role(Role::User)
            .content(InputContent::TextInput(user.to_string()))
            .build()?
    );


    // соберём объект запроса (Responses API)
    let mut input :Vec<InputItem> = vec![ system_msg, user_msg ];

    if let Some(p) = file {
        let bytes = tokio::fs::read(p).await?;

        let contents = fs::read_to_string(p)?;
        tracing::debug!("user_file contents: {:?}", &contents);
        

        let file_contents = InputFileArgs::default()
            .filename(p.to_str().unwrap())
            .file_data(contents)
            .build()?;

        let user_file = InputItem::Message(
            InputMessageArgs::default()
            .role(Role::User)
            .content(InputContent::InputItemContentList(vec![ContentType::InputFile(file_contents)]))
            .build()?
        );

        tracing::debug!("user_file: {:?}", &user_file);
        input.push(user_file);
    }


    let args = CreateResponseArgs::default()
        .model(model)
        .max_output_tokens(max_output as u32)
        .input(Input::Items(input))
        .build()?;


    // лог в /tmp
    let ts = OffsetDateTime::now_utc().unix_timestamp();
    let req_path  = format!("/tmp/gptcli-explain-req-{}-{}.json", model, ts);
    let resp_path = format!("/tmp/gptcli-explain-resp-{}-{}.json", model, ts);
    fs::write(&req_path, serde_json::to_vec_pretty(&args)?)?;

    let client = Client::new();
    let resp = client.responses().create(args).await?;
    fs::write(&resp_path, serde_json::to_vec_pretty(&resp)?)?;

    let text = extract_output_text(&resp);

    Ok((text, resp.usage.clone(), req_path, resp_path))
}

