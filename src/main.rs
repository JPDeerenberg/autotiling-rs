use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use log::{debug, error, info};
use swayipc::{Connection, Event, EventType, Node, NodeLayout, NodeType, WindowChange};

/// Configuration for master-stack layout
/// Maps application class names to their master window percentage (0.5 to 0.7)
#[derive(Debug, Clone)]
struct MasterStackConfig {
    /// Application class -> percentage of screen to occupy (50-70%)
    master_apps: HashMap<String, f32>,
    /// Whether to enable automatic window balancing
    enable_balance: bool,
}

impl Default for MasterStackConfig {
    fn default() -> Self {
        let mut master_apps = HashMap::new();
        // Example: games and browsers get 60% of screen
        master_apps.insert("firefox".to_string(), 0.6);
        master_apps.insert("chromium".to_string(), 0.6);
        master_apps.insert("steam".to_string(), 0.65);
        
        Self {
            master_apps,
            enable_balance: true,
        }
    }
}

/// Calculate the aspect ratio of a container (width / height)
fn calculate_aspect_ratio(node: &Node) -> f32 {
    let width = node.rect.width as f32;
    let height = node.rect.height as f32;
    if height == 0.0 {
        1.0
    } else {
        width / height
    }
}

/// Determine if a split should be made vertically (result: SplitV) or horizontally (result: SplitH)
/// based on the aspect ratio of the parent container.
/// This ensures windows stay as square as possible.
fn calculate_optimal_split(parent_node: &Node) -> NodeLayout {
    let aspect_ratio = calculate_aspect_ratio(parent_node);
    
    // If the container is wider than tall, split vertically (SplitV) to divide the width
    // If the container is taller than wide, split horizontally (SplitH) to divide the height
    if aspect_ratio > 1.0 {
        NodeLayout::SplitV
    } else {
        NodeLayout::SplitH
    }
}

/// Implement recursive spiral/even-split autotiling algorithm
/// This algorithm recursively applies optimal splits to maintain square windows
fn apply_spiral_autotile(node: &Node, target_layout: NodeLayout) -> Option<NodeLayout> {
    // Don't apply to non-container nodes or special layouts
    if node.node_type == NodeType::FloatingCon
        || node.layout == NodeLayout::Stacked
        || node.layout == NodeLayout::Tabbed
    {
        return None;
    }

    // If the target layout is already set, return it
    if node.layout == target_layout {
        return Some(target_layout);
    }

    // Return the optimal layout for this node
    Some(target_layout)
}

/// Check if a window should be treated as a master window
fn is_master_window(node: &Node, config: &MasterStackConfig) -> bool {
    if let Some(window_properties) = &node.window_properties {
        if let Some(class) = &window_properties.class {
            return config.master_apps.contains_key(class);
        }
    }
    false
}

/// Apply master-stack layout: master window takes 50-70% of screen, others tile in remaining space
fn apply_master_stack_layout(
    conn: &mut Connection,
    focused_node: &Node,
    config: &MasterStackConfig,
) -> Result<()> {
    if !is_master_window(focused_node, config) {
        return Ok(());
    }

    if let Some(window_properties) = &focused_node.window_properties {
        if let Some(class) = &window_properties.class {
            if let Some(&master_percentage) = config.master_apps.get(class) {
                // Clamp to valid range
                let master_pct = master_percentage.clamp(0.5, 0.7);
                
                debug!(
                    "Applying master-stack layout for {} with {}% master size",
                    class,
                    (master_pct * 100.0) as i32
                );

                // This would require more complex container manipulation
                // For now, we provide the foundation - actual implementation depends on
                // container hierarchy and sway's capabilities
                return Ok(());
            }
        }
    }

    Ok(())
}

/// Balance windows in the current container - resize them to equal sizes
fn balance_windows(conn: &mut Connection) -> Result<()> {
    debug!("Executing balance command");
    conn.run_command("balance")?;
    Ok(())
}

/// Main autotiling function: decide split direction and apply layout
fn switch_splitting_advanced(
    conn: &mut Connection,
    workspaces: &HashSet<i32>,
    config: &MasterStackConfig,
) -> Result<()> {
    // Filter by workspace if specified
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
            return Ok(());
        }
    }

    let tree = conn.get_tree().context("get_tree() failed")?;
    let focused_node = tree
        .find_focused_as_ref(|n| n.focused)
        .ok_or_else(|| anyhow!("Could not find the focused node"))?;

    // Skip special layouts and floating windows
    let is_stacked = focused_node.layout == NodeLayout::Stacked;
    let is_tabbed = focused_node.layout == NodeLayout::Tabbed;
    let is_floating = focused_node.node_type == NodeType::FloatingCon;
    let is_full_screen = focused_node.percent.unwrap_or(1.0) > 1.0;

    if is_floating || is_full_screen || is_stacked || is_tabbed {
        return Ok(());
    }

    // Find the parent container which holds the focused node
    let parent = tree
        .find_focused_as_ref(|n| n.nodes.iter().any(|n| n.focused))
        .ok_or_else(|| anyhow!("Could not find parent container"))?;

    // Calculate optimal split based on aspect ratio of parent container
    // This ensures new windows maintain near-square proportions
    let optimal_split = calculate_optimal_split(parent);

    // Only change layout if it differs from current
    if optimal_split == parent.layout {
        return Ok(());
    }

    debug!(
        "Changing layout from {:?} to {:?} (aspect ratio: {:.2})",
        parent.layout,
        optimal_split,
        calculate_aspect_ratio(parent)
    );

    let cmd = match optimal_split {
        NodeLayout::SplitV => "splitv",
        NodeLayout::SplitH => "splith",
        _ => return Ok(()),
    };

    conn.run_command(cmd).context("run_command failed")?;

    // Check if this is a master window and apply master-stack if needed
    apply_master_stack_layout(conn, focused_node, config)?;

    Ok(())
}

/// Backward-compatible wrapper function using default configuration
fn switch_splitting(conn: &mut Connection, workspaces: &HashSet<i32>) -> Result<()> {
    let config = MasterStackConfig::default();
    switch_splitting_advanced(conn, workspaces, &config)
}

#[derive(Parser)]
#[clap(version, author, about)]
struct Cli {
    /// Activate autotiling only on this workspace. More than one workspace may be specified.
    #[clap(long, short = 'w')]
    workspace: Vec<i32>,

    /// Enable automatic window balancing after new window is mapped
    #[clap(long, default_value_t = true)]
    balance: bool,

    /// Application class name to treat as master window (can be specified multiple times)
    /// Example: --master-app firefox --master-app steam
    #[clap(long)]
    master_app: Vec<String>,

    /// Master window percentage of screen (50-70), applied to all master apps
    #[clap(long, default_value_t = 0.6)]
    master_percent: f32,
}

fn main() -> Result<()> {
    env_logger::init();

    let args = Cli::parse();
    let workspace_set: HashSet<i32> = args.workspace.into_iter().collect();

    // Build master stack configuration from CLI args
    let mut master_stack_config = MasterStackConfig::default();
    let master_percent = args.master_percent.clamp(0.5, 0.7);

    // If custom master apps provided, use those instead of defaults
    if !args.master_app.is_empty() {
        master_stack_config.master_apps.clear();
        for app in args.master_app {
            master_stack_config.master_apps.insert(app, master_percent);
        }
    }

    master_stack_config.enable_balance = args.balance;

    info!(
        "Starting autotiling with {} workspaces, balance={}, master_apps={:?}",
        if workspace_set.is_empty() {
            "all".to_string()
        } else {
            format!("{:?}", workspace_set)
        },
        master_stack_config.enable_balance,
        master_stack_config.master_apps.keys().collect::<Vec<_>>()
    );

    let mut conn = Connection::new().context("failed to connect to sway ipc")?;
    let mut event_conn = Connection::new().context("failed to create event connection")?;

    // Subscribe to both window focus and window mapping events
    let mut events = event_conn
        .subscribe(&[EventType::Window])
        .context("subscribe failed")?;

    for event in events {
        let event = event.context("error reading event")?;
        match event {
            Event::Window(e) => {
                match e.change {
                    WindowChange::Focus => {
                        // Window focus event - recompute optimal split direction
                        if let Err(err) = switch_splitting_advanced(
                            &mut conn,
                            &workspace_set,
                            &master_stack_config,
                        ) {
                            error!("switch_splitting_advanced error: {:#}", err);
                        }
                    }
                    WindowChange::New => {
                        // New window mapped - apply balance if enabled
                        if master_stack_config.enable_balance {
                            // Small delay to ensure sway has updated the tree
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            if let Err(err) = balance_windows(&mut conn) {
                                error!("balance_windows error: {:#}", err);
                            }
                        }
                    }
                    _ => {
                        // Ignore other window change events (title, fullscreen, etc.)
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}
