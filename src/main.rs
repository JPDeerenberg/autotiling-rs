use std::collections::HashSet;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use log::error;
use swayipc::{Connection, Event, EventType, NodeLayout, NodeType, WindowChange};

fn switch_splitting(conn: &mut Connection, workspaces: &HashSet<i32>) -> Result<()> {
    // If a set of workspaces was provided, ensure the focused workspace is allowed.
    if !workspaces.is_empty() {
        let focused = conn
            .get_workspaces()
            .context("get_workspaces() failed")?
            .into_iter()
            .find(|w| w.focused)
            .map(|w| w.num);

        if let Some(num) = focused {
            if !workspaces.contains(&num) {
                return Ok(());
            }
        } else {
            // No focused workspace found — nothing to do.
            return Ok(());
        }
    }

    // get info from focused node and parent node which unfortunately requires us to call get_tree
    let tree = conn.get_tree().context("get_tree() failed")?;
    let focused_node = tree
        .find_focused_as_ref(|n| n.focused)
        .ok_or_else(|| anyhow!("Could not find the focused node"))?;

    // get info from the focused child node
    let is_stacked = focused_node.layout == NodeLayout::Stacked;
    let is_tabbed = focused_node.layout == NodeLayout::Tabbed;
    let is_floating = focused_node.node_type == NodeType::FloatingCon;
    let is_full_screen = focused_node.percent.unwrap_or(1.0) > 1.0;
    if is_floating || is_full_screen || is_stacked || is_tabbed {
        return Ok(());
    }

    let parent = tree
        .find_focused_as_ref(|n| n.nodes.iter().any(|n| n.focused))
        .ok_or_else(|| anyhow!("No parent"))?;

    // Decide split direction based on parent container's aspect ratio
    // to distribute space more evenly among children
    let new_layout = if parent.rect.height > parent.rect.width {
        NodeLayout::SplitV
    } else {
        NodeLayout::SplitH
    };

    if new_layout == parent.layout {
        return Ok(());
    }

    let cmd = match new_layout {
        NodeLayout::SplitV => "splitv",
        NodeLayout::SplitH => "splith",
        _ => "nop",
    };

    conn.run_command(cmd).context("run_command failed")?;
    Ok(())
}

#[derive(Parser)]
#[clap(version, author, about)]
struct Cli {
    /// Activate autotiling only on this workspace. More than one workspace may be specified.
    #[clap(long, short = 'w')]
    workspace: Vec<i32>,
}

fn main() -> Result<()> {
    env_logger::init();

    let args = Cli::parse();
    let workspace_set: HashSet<i32> = args.workspace.into_iter().collect();

    let mut conn = Connection::new().context("failed to connect to sway ipc")?;
    let mut event_conn = Connection::new().context("failed to create event connection")?;

    let mut events = event_conn
        .subscribe(&[EventType::Window])
        .context("subscribe failed")?;

    for event in events {
        let event = event.context("error reading event")?;
        match event {
            Event::Window(e) => {
                if let WindowChange::Focus = e.change {
                    // We can not use the e.container because the data is stale.
                    // If we compare that node data with the node given from get_tree() after we
                    // delete a node we find that the e.container.rect.height and e.container.rect.width are stale,
                    // and therefore we make the wrong decision on which layout our next window should be.
                    // Refer to https://github.com/swaywm/sway/issues/5873
                    if let Err(err) = switch_splitting(&mut conn, &workspace_set) {
                        error!("switch_splitting error: {:#}", err);
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}
