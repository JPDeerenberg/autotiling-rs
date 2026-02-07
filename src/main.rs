use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use clap::Parser;
use log::{debug, error, info};
use swayipc::{Connection, Event, EventType, Node, NodeLayout, NodeType, WindowChange};

/// Configuration for the autotiler
#[derive(Debug, Clone)]
struct AutoTileConfig {
    workspaces: HashSet<i32>,
    enable_balance: bool,
}

/// Calculate the aspect ratio of a container (width / height)
fn calculate_aspect_ratio(node: &Node) -> f32 {
    let width = node.rect.width as f32;
    let height = node.rect.height as f32;
    if height == 0.0 {
        1.0 // Avoid division by zero, though physics usually prevents 0 height windows
    } else {
        width / height
    }
}

/// The actual brains of the operation.
/// Determines if we should split Horizontally or Vertically based on the *Focused* node.
fn update_split_direction(conn: &mut Connection, config: &AutoTileConfig) -> Result<()> {
    // 1. Get the tree to find what we are looking at
    let tree = conn.get_tree().context("get_tree() failed")?;
    
    // 2. Find the focused node
    let focused_node = match tree.find_focused_as_ref(|n| n.focused) {
        Some(node) => node,
        None => return Ok(()), // No focus, nothing to do
    };

    // 3. Check workspace filter
    if !config.workspaces.is_empty() {
        // We have to walk up to find the workspace or check current workspace
        // Simpler approach: verify current workspace
        let workspaces = conn.get_workspaces()?;
        if let Some(focused_ws) = workspaces.iter().find(|w| w.focused) {
            if !config.workspaces.contains(&focused_ws.num) {
                return Ok(());
            }
        }
    }

    // 4. Skip floating, tabbed, stacked, or fullscreen windows
    // We don't want to mess with manual layouts
    if focused_node.node_type == NodeType::FloatingCon
        || focused_node.layout == NodeLayout::Stacked
        || focused_node.layout == NodeLayout::Tabbed
        || focused_node.percent.unwrap_or(0.0) > 1.0 
    {
        return Ok(());
    }

    // 5. Calculate Aspect Ratio of the FOCUSED node (not the parent!)
    // If we are Wide (>1.0), we want the NEXT window to be to the side -> SplitH
    // If we are Tall (<1.0), we want the NEXT window to be below -> SplitV
    let ratio = calculate_aspect_ratio(focused_node);
    
    // In Sway:
    // "splith" = Split Horizontal = Children arranged Left-to-Right
    // "splitv" = Split Vertical = Children arranged Top-to-Bottom
    
    let desired_layout = if ratio > 1.1 {
        // Wide window: Split it horizontally so the new one goes next to it
        "splith"
    } else {
        // Tall window: Split it vertically so the new one goes below
        "splitv"
    };

    debug!("Node {} Ratio: {:.2} -> Command: {}", focused_node.id, ratio, desired_layout);
    
    // Only run the command. Sway is smart enough not to break things if we spam it,
    // but ideally we'd check the current split status. 
    // However, 'split' commands set the split for the *future* window or the *current* container structure.
    conn.run_command(desired_layout).context("Failed to set split")?;

    Ok(())
}

fn balance_siblings(conn: &mut Connection) -> Result<()> {
    // This runs 'balance' which equalizes the size of siblings in the current container
    conn.run_command("balance")?;
    Ok(())
}

#[derive(Parser)]
#[clap(version, author, about)]
struct Cli {
    /// Activate autotiling only on this workspace.
    #[clap(long, short = 'w')]
    workspace: Vec<i32>,

    /// Enable automatic window balancing (run 'balance' on new windows)
    #[clap(long, default_value_t = true)]
    balance: bool,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Cli::parse();
    
    let config = AutoTileConfig {
        workspaces: args.workspace.into_iter().collect(),
        enable_balance: args.balance,
    };

    info!("Jarvis Autotiling initialized. Workspaces: {:?}, Balance: {}", 
        config.workspaces, config.enable_balance);

    // Connect to Sway
    let mut conn = Connection::new().context("Failed to connect to Sway IPC")?;
    
    // Subscribe to Window events. 
    // THIS is how you do it, Tony. No more 'while loop sleep'.
    let events = Connection::new()
        .context("Failed to open subscription connection")?
        .subscribe(&[EventType::Window])
        .context("Failed to subscribe to window events")?;

    // Initial pass: fix the currently focused window immediately
    if let Err(e) = update_split_direction(&mut conn, &config) {
        error!("Initial setup failed: {}", e);
    }

    // Event Loop
    for event in events {
        match event {
            Ok(Event::Window(e)) => {
                match e.change {
                    WindowChange::Focus => {
                        // When focus changes, we determine how the *next* window should open
                        // based on the dimensions of the window we just focused.
                        if let Err(err) = update_split_direction(&mut conn, &config) {
                            error!("Error handling focus: {}", err);
                        }
                    }
                    WindowChange::New => {
                        // A new window just appeared. 
                        // It will inherit the split we set on the previous 'Focus' event.
                        // Now we set the split for *this* new window (recursion).
                        if let Err(err) = update_split_direction(&mut conn, &config) {
                            error!("Error handling new window: {}", err);
                        }

                        // If enabled, balance the container so everything looks pretty
                        if config.enable_balance {
                            if let Err(err) = balance_siblings(&mut conn) {
                                error!("Error balancing: {}", err);
                            }
                        }
                    }
                    WindowChange::Close => {
                        // If a window closes, re-balance the survivors
                        if config.enable_balance {
                            if let Err(err) = balance_siblings(&mut conn) {
                                error!("Error balancing: {}", err);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(_) => {} // Ignore other events
            Err(e) => {
                error!("Event stream error: {}", e);
                break; 
            }
        }
    }

    Ok(())
}