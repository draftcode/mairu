#[derive(clap::Args)]
pub struct ListSessionsArgs {
    /// Show time in UTC instead of local time
    #[arg(long, default_value_t = false)]
    pub utc: bool,

    #[arg(long, short = 'o', value_enum, default_value_t = ListSessionsFormat::Text)]
    output: ListSessionsFormat,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
enum ListSessionsFormat {
    Text,
    Json,
}

#[derive(serde::Serialize)]
struct JsonSession<'a> {
    id: u32,
    server_id: &'a str,
    server_url: &'a str,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    refreshable: bool,
}

#[tokio::main]
pub async fn run(args: &ListSessionsArgs) -> Result<(), anyhow::Error> {
    let mut agent = crate::cmd::agent::connect_or_start().await?;
    let list = agent
        .list_sessions(tonic::Request::new(crate::proto::ListSessionsRequest {}))
        .await?
        .into_inner();

    match args.output {
        ListSessionsFormat::Text => {
            for session in list.sessions.iter() {
                print_session(session, args.utc);
            }
        }
        ListSessionsFormat::Json => {
            let sessions: Vec<_> = list
                .sessions
                .iter()
                .map(|s| JsonSession {
                    id: s.id,
                    server_id: &s.server_id,
                    server_url: &s.server_url,
                    expires_at: s.expiration().ok().flatten(),
                    refreshable: s.refreshable,
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({ "sessions": sessions }))?
            );
        }
    }

    Ok(())
}

pub(crate) fn print_session(session: &crate::proto::Session, utc: bool) {
    let expiring = match session.expiration() {
        Ok(Some(e)) => {
            let t = format_time(e, utc);
            if session.refreshable {
                format!(" [renews after {t}]")
            } else {
                format!(" [until {t}]")
            }
        }
        Ok(None) => {
            if session.refreshable {
                "[refreshable]".to_string()
            } else {
                "".to_string()
            }
        }
        Err(e) => {
            tracing::warn!(err = ?e, session = ?session, "Invalid expiration timestamp");
            "".to_string()
        }
    };
    println!(
        "{}. {}: {}{}",
        session.id, session.server_id, session.server_url, expiring
    );
}

fn format_time(t: chrono::DateTime<chrono::Utc>, utc: bool) -> String {
    if utc {
        t.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    } else {
        chrono::DateTime::<chrono::Local>::from(t)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    }
}
