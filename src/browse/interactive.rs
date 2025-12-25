//! Interactive TUI for browsing and selecting music

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    prelude::*,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::collections::HashSet;
use std::io;
use tokio::sync::mpsc;
use tracing::debug;

use crate::device::{Device, DeviceDetector, UnmountedDevice};
use crate::subsonic::{Album, Artist, Playlist, SubsonicClient, SyncSelection};
use crate::sync::{SyncEngine, SyncProgress as SyncProgressEvent};

/// Current view in the browser
#[derive(Debug, Clone, PartialEq)]
pub enum BrowseView {
    Artists,
    Albums { artist_id: String, artist_name: String },
    AlbumTracks { album: Album },
    Playlists,
    PlaylistTracks { playlist: Playlist },
    DeviceSelection,
    SyncProgress,
}

/// Progress info for syncing
#[derive(Debug, Clone, Default)]
pub struct SyncProgressInfo {
    pub current_album: String,
    pub current_artist: String,
    pub albums_completed: usize,
    pub albums_total: usize,
    pub tracks_completed: usize,
    pub tracks_total: usize,
    pub bytes_downloaded: u64,
    pub is_complete: bool,
    pub error: Option<String>,
    pub log_messages: Vec<String>,
}

/// Result from the browser - either just a selection or a selection + device
#[derive(Debug)]
pub enum BrowseResult {
    SelectionOnly(SyncSelection),
    SyncToDevice { selection: SyncSelection, device: Device },
}

/// Browser state
struct BrowserState {
    view: BrowseView,
    artists: Vec<Artist>,
    albums: Vec<Album>,
    playlists: Vec<Playlist>,
    mounted_devices: Vec<Device>,
    unmounted_devices: Vec<UnmountedDevice>,
    list_state: ListState,
    selected_albums: HashSet<String>,
    selected_playlists: HashSet<String>,
    status_message: String,
    sync_progress: SyncProgressInfo,
    selected_device: Option<Device>,
    /// Receiver for sync progress events
    progress_rx: Option<mpsc::Receiver<SyncProgressEvent>>,
    /// Selection being synced
    sync_selection: Option<SyncSelection>,
}

impl BrowserState {
    fn new(view: BrowseView) -> Self {
        let mut list_state = ListState::default();
        list_state.select(Some(0));

        Self {
            view,
            artists: Vec::new(),
            albums: Vec::new(),
            playlists: Vec::new(),
            mounted_devices: Vec::new(),
            unmounted_devices: Vec::new(),
            list_state,
            selected_albums: HashSet::new(),
            selected_playlists: HashSet::new(),
            status_message: String::new(),
            sync_progress: SyncProgressInfo::default(),
            selected_device: None,
            progress_rx: None,
            sync_selection: None,
        }
    }

    fn current_list_len(&self) -> usize {
        match &self.view {
            BrowseView::Artists => self.artists.len(),
            BrowseView::Albums { .. } => self.albums.len(),
            BrowseView::AlbumTracks { album } => album.song_count.unwrap_or(0) as usize,
            BrowseView::Playlists => self.playlists.len(),
            BrowseView::PlaylistTracks { playlist } => playlist.song_count.unwrap_or(0) as usize,
            BrowseView::DeviceSelection => self.mounted_devices.len() + self.unmounted_devices.len(),
            BrowseView::SyncProgress => self.sync_progress.log_messages.len(),
        }
    }

    fn total_devices(&self) -> usize {
        self.mounted_devices.len() + self.unmounted_devices.len()
    }

    fn move_up(&mut self) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }

        let i = match self.list_state.selected() {
            Some(i) => {
                if i == 0 {
                    len - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }

    fn move_down(&mut self) {
        let len = self.current_list_len();
        if len == 0 {
            return;
        }

        let i = match self.list_state.selected() {
            Some(i) => {
                if i >= len - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.list_state.select(Some(i));
    }
}

/// Run the interactive browser
pub async fn run_browser(client: &SubsonicClient, initial_view: BrowseView) -> Result<BrowseResult> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create state
    let mut state = BrowserState::new(initial_view.clone());

    // Load initial data
    state.status_message = "Loading...".to_string();
    match &initial_view {
        BrowseView::Artists | BrowseView::Albums { .. } | BrowseView::AlbumTracks { .. } => {
            state.artists = client.get_artists().await?;
        }
        BrowseView::Playlists | BrowseView::PlaylistTracks { .. } => {
            state.playlists = client.get_playlists().await?;
        }
        BrowseView::DeviceSelection | BrowseView::SyncProgress => {
            // Load devices if starting in device selection (shouldn't happen normally)
            state.mounted_devices = DeviceDetector::scan().await.unwrap_or_default();
            state.unmounted_devices = DeviceDetector::scan_unmounted().await.unwrap_or_default();
        }
    }
    state.status_message.clear();

    // Main loop
    let result = run_browser_loop(&mut terminal, &mut state, client).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}

async fn run_browser_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut BrowserState,
    client: &SubsonicClient,
) -> Result<BrowseResult> {
    loop {
        // Poll for sync progress updates if we're syncing
        if state.view == BrowseView::SyncProgress {
            // Collect events first to avoid double borrow
            let events: Vec<SyncProgressEvent> = {
                if let Some(rx) = &mut state.progress_rx {
                    let mut events = Vec::new();
                    while let Ok(event) = rx.try_recv() {
                        events.push(event);
                    }
                    events
                } else {
                    Vec::new()
                }
            };
            for event in events {
                handle_sync_progress_event(state, event);
            }
        }

        // Draw UI
        terminal.draw(|f| draw_ui(f, state))?;

        // Handle input
        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if state.view == BrowseView::DeviceSelection {
                            // Go back to previous view
                            state.view = BrowseView::Artists;
                            state.list_state.select(Some(0));
                        } else if state.view == BrowseView::SyncProgress {
                            if state.sync_progress.is_complete {
                                // Sync done, return with result
                                if let (Some(selection), Some(device)) =
                                    (state.sync_selection.take(), state.selected_device.take())
                                {
                                    return Ok(BrowseResult::SyncToDevice { selection, device });
                                }
                                return Ok(BrowseResult::SelectionOnly(build_selection(state, client).await?));
                            }
                            // Don't allow quitting during sync
                        } else {
                            // Return selection without device
                            return Ok(BrowseResult::SelectionOnly(build_selection(state, client).await?));
                        }
                    }
                    KeyCode::Char('s') => {
                        // Show device selection
                        if state.view != BrowseView::DeviceSelection && state.view != BrowseView::SyncProgress {
                            let selection = build_selection(state, client).await?;
                            if selection.is_empty() {
                                state.status_message = "No items selected!".to_string();
                            } else {
                                // Load devices and switch to device selection view
                                state.status_message = "Loading devices...".to_string();
                                terminal.draw(|f| draw_ui(f, state))?;

                                state.mounted_devices = DeviceDetector::scan().await.unwrap_or_default();
                                state.unmounted_devices = DeviceDetector::scan_unmounted().await.unwrap_or_default();
                                state.status_message.clear();

                                if state.total_devices() == 0 {
                                    state.status_message = "No devices found! Connect a device and try again.".to_string();
                                } else {
                                    state.view = BrowseView::DeviceSelection;
                                    state.list_state.select(Some(0));
                                }
                            }
                        }
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        if state.view != BrowseView::SyncProgress {
                            state.move_up();
                        }
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        if state.view != BrowseView::SyncProgress {
                            state.move_down();
                        }
                    }
                    KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                        if state.view == BrowseView::DeviceSelection {
                            // Select device and start sync
                            handle_device_select(state, client).await?;
                        } else if state.view != BrowseView::SyncProgress {
                            handle_enter(state, client).await?;
                        }
                    }
                    KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                        if state.view == BrowseView::DeviceSelection {
                            state.view = BrowseView::Artists;
                            state.list_state.select(Some(0));
                        } else if state.view != BrowseView::SyncProgress {
                            handle_back(state, client).await?;
                        }
                    }
                    KeyCode::Char(' ') => {
                        if state.view != BrowseView::SyncProgress {
                            handle_toggle(state);
                        }
                    }
                    KeyCode::Char('a') => {
                        if state.view != BrowseView::SyncProgress {
                            handle_select_all(state);
                        }
                    }
                    KeyCode::Tab => {
                        if state.view != BrowseView::DeviceSelection && state.view != BrowseView::SyncProgress {
                            handle_tab(state, client).await?;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Handle a sync progress event
fn handle_sync_progress_event(state: &mut BrowserState, event: SyncProgressEvent) {
    match event {
        SyncProgressEvent::Started { total_albums, total_playlists } => {
            state.sync_progress.albums_total = total_albums;
            state.sync_progress.log_messages.push(format!(
                "Starting sync: {} albums, {} playlists",
                total_albums, total_playlists
            ));
        }
        SyncProgressEvent::AlbumStarted { artist, album, track_count } => {
            state.sync_progress.current_artist = artist.clone();
            state.sync_progress.current_album = album.clone();
            state.sync_progress.tracks_completed = 0;
            state.sync_progress.tracks_total = track_count;
            state.sync_progress.log_messages.push(format!(
                "Syncing: {} - {} ({} tracks)",
                artist, album, track_count
            ));
        }
        SyncProgressEvent::TrackCompleted { track_num, total_tracks } => {
            state.sync_progress.tracks_completed = track_num;
            state.sync_progress.tracks_total = total_tracks;
        }
        SyncProgressEvent::AlbumCompleted { artist, album } => {
            state.sync_progress.albums_completed += 1;
            state.sync_progress.log_messages.push(format!(
                "  Completed: {} - {}",
                artist, album
            ));
        }
        SyncProgressEvent::AlbumSkipped { artist, album } => {
            state.sync_progress.albums_completed += 1;
            state.sync_progress.log_messages.push(format!(
                "  Skipped (already synced): {} - {}",
                artist, album
            ));
        }
        SyncProgressEvent::PlaylistStarted { name, track_count } => {
            state.sync_progress.current_album = name.clone();
            state.sync_progress.current_artist = "Playlist".to_string();
            state.sync_progress.tracks_completed = 0;
            state.sync_progress.tracks_total = track_count;
            state.sync_progress.log_messages.push(format!(
                "Syncing playlist: {} ({} tracks)",
                name, track_count
            ));
        }
        SyncProgressEvent::PlaylistCompleted { name } => {
            state.sync_progress.log_messages.push(format!(
                "  Completed playlist: {}",
                name
            ));
        }
        SyncProgressEvent::PlaylistSkipped { name } => {
            state.sync_progress.log_messages.push(format!(
                "  Skipped playlist (already synced): {}",
                name
            ));
        }
        SyncProgressEvent::Error { message } => {
            state.sync_progress.error = Some(message.clone());
            state.sync_progress.log_messages.push(format!("ERROR: {}", message));
        }
        SyncProgressEvent::Complete { albums_synced, playlists_synced, tracks_downloaded, bytes_downloaded } => {
            state.sync_progress.is_complete = true;
            state.sync_progress.bytes_downloaded = bytes_downloaded;
            let mb = bytes_downloaded as f64 / 1_048_576.0;
            state.sync_progress.log_messages.push(format!(
                "Sync complete! {} albums, {} playlists, {} tracks ({:.1} MB)",
                albums_synced, playlists_synced, tracks_downloaded, mb
            ));
        }
    }
}

async fn handle_device_select(state: &mut BrowserState, client: &SubsonicClient) -> Result<bool> {
    let selected = state.list_state.selected().unwrap_or(0);
    let mounted_count = state.mounted_devices.len();

    let device: Option<Device> = if selected < mounted_count {
        // Selected a mounted device
        Some(state.mounted_devices[selected].clone())
    } else {
        // Selected an unmounted device - mount it first
        let unmounted_idx = selected - mounted_count;
        if let Some(unmounted) = state.unmounted_devices.get(unmounted_idx) {
            state.status_message = format!("Mounting {}...", unmounted.label.as_deref().unwrap_or(&unmounted.name));

            match DeviceDetector::mount(&unmounted.name).await {
                Ok(_mount_point) => {
                    // Re-scan to get the mounted device
                    state.mounted_devices = DeviceDetector::scan().await.unwrap_or_default();

                    // Find the newly mounted device
                    state.mounted_devices.iter().find(|d| d.name == unmounted.name).cloned()
                }
                Err(e) => {
                    state.status_message = format!("Mount failed: {}", e);
                    return Ok(false);
                }
            }
        } else {
            None
        }
    };

    let Some(device) = device else {
        return Ok(false);
    };

    // Build selection and start sync
    let selection = build_selection(state, client).await?;

    // Create progress channel
    let (tx, rx) = mpsc::channel::<SyncProgressEvent>(100);

    // Store state for sync
    state.selected_device = Some(device.clone());
    state.sync_selection = Some(selection.clone());
    state.progress_rx = Some(rx);
    state.sync_progress = SyncProgressInfo {
        albums_total: selection.albums.len(),
        ..Default::default()
    };

    // Spawn sync task
    let device_path = device.mount_point.clone();
    let client_clone = client.clone();
    tokio::spawn(async move {
        let mut engine = match SyncEngine::new(client_clone, device_path, 4) {
            Ok(e) => e,
            Err(e) => {
                let _ = tx.send(SyncProgressEvent::Error {
                    message: format!("Failed to create sync engine: {}", e),
                }).await;
                return;
            }
        };

        if let Err(e) = engine.sync_with_progress(&selection, tx.clone()).await {
            let _ = tx.send(SyncProgressEvent::Error {
                message: format!("Sync failed: {}", e),
            }).await;
        }
    });

    // Switch to sync progress view
    state.view = BrowseView::SyncProgress;
    state.status_message.clear();

    Ok(true)
}

async fn handle_enter(state: &mut BrowserState, client: &SubsonicClient) -> Result<()> {
    let selected = state.list_state.selected().unwrap_or(0);

    match &state.view {
        BrowseView::Artists => {
            if let Some(artist) = state.artists.get(selected) {
                state.status_message = format!("Loading albums for {}...", artist.name);
                let artist_details = client.get_artist(&artist.id).await?;
                state.albums = artist_details.album;
                state.view = BrowseView::Albums {
                    artist_id: artist.id.clone(),
                    artist_name: artist.name.clone(),
                };
                state.list_state.select(Some(0));
                state.status_message.clear();
            }
        }
        BrowseView::Albums { .. } => {
            if let Some(album) = state.albums.get(selected) {
                state.view = BrowseView::AlbumTracks {
                    album: album.clone(),
                };
                state.list_state.select(Some(0));
            }
        }
        BrowseView::Playlists => {
            if let Some(playlist) = state.playlists.get(selected) {
                state.view = BrowseView::PlaylistTracks {
                    playlist: playlist.clone(),
                };
                state.list_state.select(Some(0));
            }
        }
        _ => {}
    }

    Ok(())
}

async fn handle_back(state: &mut BrowserState, _client: &SubsonicClient) -> Result<()> {
    match &state.view {
        BrowseView::Albums { .. } => {
            state.view = BrowseView::Artists;
            state.list_state.select(Some(0));
        }
        BrowseView::AlbumTracks { .. } => {
            // Go back to albums view - need to know which artist
            // For now, go to artists view
            state.view = BrowseView::Artists;
            state.list_state.select(Some(0));
        }
        BrowseView::PlaylistTracks { .. } => {
            state.view = BrowseView::Playlists;
            state.list_state.select(Some(0));
        }
        _ => {}
    }
    Ok(())
}

fn handle_toggle(state: &mut BrowserState) {
    let selected = state.list_state.selected().unwrap_or(0);

    match &state.view {
        BrowseView::Albums { .. } => {
            if let Some(album) = state.albums.get(selected) {
                if state.selected_albums.contains(&album.id) {
                    state.selected_albums.remove(&album.id);
                } else {
                    state.selected_albums.insert(album.id.clone());
                }
            }
        }
        BrowseView::Playlists => {
            if let Some(playlist) = state.playlists.get(selected) {
                if state.selected_playlists.contains(&playlist.id) {
                    state.selected_playlists.remove(&playlist.id);
                } else {
                    state.selected_playlists.insert(playlist.id.clone());
                }
            }
        }
        _ => {}
    }
}

fn handle_select_all(state: &mut BrowserState) {
    match &state.view {
        BrowseView::Albums { .. } => {
            for album in &state.albums {
                state.selected_albums.insert(album.id.clone());
            }
        }
        BrowseView::Playlists => {
            for playlist in &state.playlists {
                state.selected_playlists.insert(playlist.id.clone());
            }
        }
        _ => {}
    }
}

async fn handle_tab(state: &mut BrowserState, client: &SubsonicClient) -> Result<()> {
    match &state.view {
        BrowseView::Artists | BrowseView::Albums { .. } | BrowseView::AlbumTracks { .. } => {
            // Switch to playlists
            if state.playlists.is_empty() {
                state.status_message = "Loading playlists...".to_string();
                state.playlists = client.get_playlists().await?;
                state.status_message.clear();
            }
            state.view = BrowseView::Playlists;
            state.list_state.select(Some(0));
        }
        BrowseView::Playlists | BrowseView::PlaylistTracks { .. } => {
            // Switch to artists
            if state.artists.is_empty() {
                state.status_message = "Loading artists...".to_string();
                state.artists = client.get_artists().await?;
                state.status_message.clear();
            }
            state.view = BrowseView::Artists;
            state.list_state.select(Some(0));
        }
        BrowseView::DeviceSelection | BrowseView::SyncProgress => {
            // Don't switch views from device selection or sync progress
        }
    }
    Ok(())
}

async fn build_selection(state: &BrowserState, _client: &SubsonicClient) -> Result<SyncSelection> {
    let mut selection = SyncSelection::new();

    // Add selected albums
    for album_id in &state.selected_albums {
        // Find album in loaded albums or fetch it
        if let Some(album) = state.albums.iter().find(|a| &a.id == album_id) {
            selection.albums.push(album.clone());
        }
    }

    // Add selected playlists
    for playlist_id in &state.selected_playlists {
        if let Some(playlist) = state.playlists.iter().find(|p| &p.id == playlist_id) {
            selection.playlists.push(playlist.clone());
        }
    }

    Ok(selection)
}

/// Draw the sync progress view
fn draw_sync_progress(f: &mut Frame, state: &BrowserState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Length(4),  // Album progress
            Constraint::Length(4),  // Track progress
            Constraint::Min(5),     // Log messages
            Constraint::Length(3),  // Footer/help
        ])
        .split(f.area());

    // Header
    let device_name = state.selected_device
        .as_ref()
        .map(|d| d.label.as_deref().unwrap_or(&d.name))
        .unwrap_or("Unknown");

    let header_text = if state.sync_progress.is_complete {
        format!("Sync Complete - {}", device_name)
    } else {
        format!("Syncing to {} ...", device_name)
    };

    let header_style = if state.sync_progress.is_complete {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else if state.sync_progress.error.is_some() {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    };

    let header = Paragraph::new(header_text)
        .style(header_style)
        .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, chunks[0]);

    // Album progress gauge
    let album_progress = if state.sync_progress.albums_total > 0 {
        (state.sync_progress.albums_completed as f64 / state.sync_progress.albums_total as f64).min(1.0)
    } else {
        0.0
    };

    let album_label = format!(
        "Albums: {}/{}",
        state.sync_progress.albums_completed,
        state.sync_progress.albums_total
    );

    let album_gauge = Gauge::default()
        .block(Block::default().title("Album Progress").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Cyan))
        .percent((album_progress * 100.0) as u16)
        .label(album_label);
    f.render_widget(album_gauge, chunks[1]);

    // Track progress gauge
    let track_progress = if state.sync_progress.tracks_total > 0 {
        (state.sync_progress.tracks_completed as f64 / state.sync_progress.tracks_total as f64).min(1.0)
    } else {
        0.0
    };

    let current_item = if !state.sync_progress.current_album.is_empty() {
        format!(
            "{} - {} ({}/{})",
            state.sync_progress.current_artist,
            state.sync_progress.current_album,
            state.sync_progress.tracks_completed,
            state.sync_progress.tracks_total
        )
    } else {
        "Preparing...".to_string()
    };

    let track_gauge = Gauge::default()
        .block(Block::default().title(current_item).borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Green))
        .percent((track_progress * 100.0) as u16);
    f.render_widget(track_gauge, chunks[2]);

    // Log messages
    let visible_lines = chunks[3].height.saturating_sub(2) as usize;
    let messages: Vec<Line> = state.sync_progress.log_messages
        .iter()
        .rev()
        .take(visible_lines)
        .rev()
        .map(|msg| {
            let style = if msg.starts_with("ERROR") {
                Style::default().fg(Color::Red)
            } else if msg.starts_with("  Completed") || msg.starts_with("Sync complete") {
                Style::default().fg(Color::Green)
            } else if msg.starts_with("  Skipped") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Line::styled(msg.clone(), style)
        })
        .collect();

    let log = Paragraph::new(messages)
        .block(Block::default().title("Activity Log").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    f.render_widget(log, chunks[3]);

    // Footer
    let help_text = if state.sync_progress.is_complete {
        "Press q to finish"
    } else {
        "Syncing in progress..."
    };

    let footer = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer, chunks[4]);
}

fn draw_ui(f: &mut Frame, state: &BrowserState) {
    // Special layout for sync progress view
    if state.view == BrowseView::SyncProgress {
        draw_sync_progress(f, state);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(10),   // List
            Constraint::Length(3), // Footer/help
        ])
        .split(f.area());

    // Header
    let title = match &state.view {
        BrowseView::Artists => "Artists",
        BrowseView::Albums { artist_name, .. } => artist_name,
        BrowseView::AlbumTracks { album } => &album.name,
        BrowseView::Playlists => "Playlists",
        BrowseView::PlaylistTracks { playlist } => &playlist.name,
        BrowseView::DeviceSelection => "Select Device",
        BrowseView::SyncProgress => "Syncing...",
    };

    let selection_count = state.selected_albums.len() + state.selected_playlists.len();
    let header_text = if selection_count > 0 {
        format!("{} ({} selected)", title, selection_count)
    } else {
        title.to_string()
    };

    let header = Paragraph::new(header_text)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, chunks[0]);

    // List
    let items: Vec<ListItem> = match &state.view {
        BrowseView::Artists => state
            .artists
            .iter()
            .map(|a| {
                let album_count = a.album_count.map(|c| format!(" ({} albums)", c)).unwrap_or_default();
                ListItem::new(format!("{}{}", a.name, album_count))
            })
            .collect(),
        BrowseView::Albums { .. } => state
            .albums
            .iter()
            .map(|a| {
                let selected = state.selected_albums.contains(&a.id);
                let prefix = if selected { "[x] " } else { "[ ] " };
                let year = a.year.map(|y| format!(" ({})", y)).unwrap_or_default();
                let style = if selected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{}{}{}", prefix, a.name, year)).style(style)
            })
            .collect(),
        BrowseView::AlbumTracks { album } => {
            vec![ListItem::new(format!(
                "Album has {} tracks - press Backspace to go back",
                album.song_count.unwrap_or(0)
            ))]
        }
        BrowseView::Playlists => state
            .playlists
            .iter()
            .map(|p| {
                let selected = state.selected_playlists.contains(&p.id);
                let prefix = if selected { "[x] " } else { "[ ] " };
                let count = p.song_count.map(|c| format!(" ({} tracks)", c)).unwrap_or_default();
                let style = if selected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{}{}{}", prefix, p.name, count)).style(style)
            })
            .collect(),
        BrowseView::PlaylistTracks { playlist } => {
            vec![ListItem::new(format!(
                "Playlist has {} tracks - press Backspace to go back",
                playlist.song_count.unwrap_or(0)
            ))]
        }
        BrowseView::DeviceSelection => {
            let mut items: Vec<ListItem> = Vec::new();

            // Add mounted devices first
            for device in &state.mounted_devices {
                let label = device.label.as_deref().unwrap_or("(no label)");
                let free_gb = device.free_space as f64 / 1_073_741_824.0;
                items.push(ListItem::new(format!(
                    "  {} - {} ({:.1} GB free)",
                    device.name, label, free_gb
                )).style(Style::default().fg(Color::Green)));
            }

            // Add unmounted devices
            for device in &state.unmounted_devices {
                let label = device.label.as_deref().unwrap_or("(no label)");
                let size_gb = device.size as f64 / 1_073_741_824.0;
                items.push(ListItem::new(format!(
                    "  {} - {} ({:.1} GB, unmounted)",
                    device.name, label, size_gb
                )).style(Style::default().fg(Color::Yellow)));
            }

            if items.is_empty() {
                items.push(ListItem::new("No devices found. Connect a device and press 's' again."));
            }

            items
        }
        BrowseView::SyncProgress => {
            // This case should never be reached due to early return, but compiler needs it
            vec![]
        }
    };

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(list, chunks[1], &mut state.list_state.clone());

    // Footer/help
    let help_text = match &state.view {
        BrowseView::Artists => "↑/↓: Navigate | Enter: View albums | Tab: Playlists | q: Done",
        BrowseView::Albums { .. } => "↑/↓: Navigate | Space: Select | a: Select all | Enter: Preview | Backspace: Back | Tab: Playlists | s: Sync | q: Done",
        BrowseView::Playlists => "↑/↓: Navigate | Space: Select | a: Select all | Tab: Artists | s: Sync | q: Done",
        BrowseView::DeviceSelection => "↑/↓: Navigate | Enter: Sync to device | Backspace/q: Cancel",
        _ => "Backspace: Back | q: Done",
    };

    let footer = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer, chunks[2]);

    // Status message overlay
    if !state.status_message.is_empty() {
        let status = Paragraph::new(state.status_message.clone())
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL));
        let area = centered_rect(50, 3, f.area());
        f.render_widget(status, area);
    }
}

fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height) / 2),
            Constraint::Length(height),
            Constraint::Percentage((100 - height) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// SyncSelection persistence
impl SyncSelection {
    const SELECTION_FILE: &'static str = ".nutune-selection.json";

    pub fn save(&self) -> Result<()> {
        let path = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(Self::SELECTION_FILE);

        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        debug!("Saved selection to {}", path.display());
        Ok(())
    }

    pub fn load() -> Result<Self> {
        let path = dirs::cache_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(Self::SELECTION_FILE);

        if !path.exists() {
            return Ok(Self::new());
        }

        let content = std::fs::read_to_string(&path)?;
        let selection: Self = serde_json::from_str(&content)?;
        debug!("Loaded selection from {}", path.display());
        Ok(selection)
    }
}
