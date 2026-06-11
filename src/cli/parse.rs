// SPDX-License-Identifier: GPL-3.0-or-later
// `parse` subcommand.

use crate::cli::output::Output;
use crate::cli::root::ParseCmd;
use crate::error::CliError;
use crate::ipc::bilibili_api;
use crate::ipc::media::{parse, ResourceRef};

pub async fn run(cmd: ParseCmd, out: &Output) -> Result<(), CliError> {
    let res: ResourceRef = match &cmd {
        ParseCmd::Url { input } => parse(input)?,
        ParseCmd::Bv { id } => parse(id)?,
        ParseCmd::Av { id } => parse(id)?,
        ParseCmd::Bangumi { id } => parse(id)?,
        ParseCmd::Episode { id } => parse(id)?,
        ParseCmd::Fav { id } => parse(id)?,
        ParseCmd::Watchlater => parse("watchlater")?,
        ParseCmd::User { id } => parse(id)?,
    };
    // Try to fetch the upstream description. Network failures are
    // non-fatal — we still print the local classification.
    match bilibili_api::describe(&res).await {
        Ok(d) => out.ok(d),
        Err(e) => {
            // Fall back to the local classification.
            out.ok(serde_json::json!({
                "kind": res.kind.as_str(),
                "id": res.id,
                "fetch_error": format!("{e}"),
            }))
        }
    }
}
