// SPDX-License-Identifier: GPL-3.0-or-later
// `schedule` subcommand.

use crate::cli::output::Output;
use crate::cli::root::ScheduleCmd;
use crate::error::CliError;
use crate::ipc::media::parse;
use crate::ipc::storage::schedulers::{self, Scheduler};
use serde::Serialize;

pub async fn run(cmd: ScheduleCmd, out: &Output) -> Result<(), CliError> {
    match cmd {
        ScheduleCmd::List => cmd_list(out).await,
        ScheduleCmd::Add { cron, input } => cmd_add(&cron, &input, out).await,
        ScheduleCmd::Remove { id } => cmd_remove(&id, out).await,
        ScheduleCmd::Run { id } => cmd_run(&id, out).await,
    }
}

async fn cmd_list(out: &Output) -> Result<(), CliError> {
    let all = schedulers::load().await?;
    out.list(
        all.into_iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "cron": s.cron,
                    "source": s.source,
                    "enabled": s.enabled,
                    "last_run": s.last_run,
                    "next_run": s.next_run,
                })
            })
            .collect(),
    )
}

#[derive(Serialize)]
struct AddOut {
    id: String,
    cron: String,
    source: String,
}

async fn cmd_add(cron: &str, input: &str, out: &Output) -> Result<(), CliError> {
    // Validate cron via the `cron` crate.
    cron::Schedule::from_str(cron)
        .map_err(|e| CliError::msg(format!("invalid cron: {e}")))?;
    // Validate the source.
    let _ = parse(input)?;
    let s = Scheduler {
        id: uuid::Uuid::new_v4().to_string(),
        cron: cron.to_string(),
        source: input.to_string(),
        options: serde_json::json!({}),
        enabled: true,
        last_run: None,
        next_run: None,
        created_at: crate::ipc::shared::get_sec(),
    };
    schedulers::insert(&s).await?;
    out.ok(AddOut {
        id: s.id,
        cron: s.cron,
        source: s.source,
    })
}

async fn cmd_remove(id: &str, out: &Output) -> Result<(), CliError> {
    schedulers::remove(id).await?;
    out.status(format!("scheduler {id} removed"))
}

async fn cmd_run(id: &str, out: &Output) -> Result<(), CliError> {
    let s = schedulers::get(id)
        .await?
        .ok_or_else(|| CliError::msg(format!("scheduler {id} not found")))?;
    let mut updated = s.clone();
    updated.last_run = Some(crate::ipc::shared::get_sec());
    schedulers::update(&updated).await?;
    out.ok(serde_json::json!({
        "id": id,
        "source": s.source,
        "action": "queued for immediate execution",
    }))
}

use std::str::FromStr;
