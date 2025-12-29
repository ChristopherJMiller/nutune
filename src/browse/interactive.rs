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
    text::Line,
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use std::collections::HashSet;
use std::io;
use tokio::sync::mpsc;
use tracing::debug;

use crate::device::{Device, DeviceDetector, SyncManifest, UnmountedDevice};
use crate::subsonic::{Album, Artist, Playlist, SubsonicClient, SyncSelection};
use crate::sync::{DeletionSelection, SyncEngine, SyncProgress as SyncProgressEvent};

/// Current view in the browser
#[derive(Debug, Clone, PartialEq)]
pub enum BrowseView {
    Artists,
    Albums { artist_id: String, artist_name: String },
    AlbumTracks { album: Album },
    Playlists,
    PlaylistTracks { playlist: Playlist },
    DeviceSelection,
    SyncConfirmation,
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
    /// Artists with all albums selected (for display purposes)
    selected_artists: HashSet<String>,
    /// Cache of album IDs per artist for quick lookup
    artist_album_ids: std::collections::HashMap<String, Vec<String>>,
    /// Cache of Album objects by ID for selection building
    album_cache: std::collections::HashMap<String, Album>,
    status_message: String,
    /// When the status message was set (for auto-clear timeout)
    status_message_time: Option<std::time::Instant>,
    sync_progress: SyncProgressInfo,
    selected_device: Option<Device>,
    /// Receiver for sync progress events
    progress_rx: Option<mpsc::Receiver<SyncProgressEvent>>,
    /// Selection being synced
    sync_selection: Option<SyncSelection>,
    /// Deletions pending for sync
    pending_deletions: Option<DeletionSelection>,
    /// Albums already synced to device (from manifest)
    synced_album_ids: HashSet<String>,
    /// Playlists already synced to device (from manifest)
    synced_playlist_ids: HashSet<String>,
    /// Active device for sync status display
    active_device: Option<Device>,
    /// Search/filter mode
    search_mode: bool,
    /// Current search query
    search_query: String,
    /// Filtered indices (maps display index to original index)
    filtered_indices: Vec<usize>,
    /// Show help overlay
    show_help: bool,
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
            selected_artists: HashSet::new(),
            artist_album_ids: std::collections::HashMap::new(),
            album_cache: std::collections::HashMap::new(),
            status_message: String::new(),
            status_message_time: None,
            sync_progress: SyncProgressInfo::default(),
            selected_device: None,
            progress_rx: None,
            sync_selection: None,
            pending_deletions: None,
            synced_album_ids: HashSet::new(),
            synced_playlist_ids: HashSet::new(),
            active_device: None,
            search_mode: false,
            search_query: String::new(),
            filtered_indices: Vec::new(),
            show_help: false,
        }
    }

    /// Load synced content from a device's manifest
    fn load_synced_content(&mut self, device: &Device) {
        if let Ok(Some(manifest)) = crate::device::SyncManifest::load(&device.mount_point) {
            self.synced_album_ids = manifest.synced_albums.iter().map(|a| a.id.clone()).collect();
            self.synced_playlist_ids = manifest.synced_playlists.iter().map(|p| p.id.clone()).collect();
            self.active_device = Some(device.clone());
        }
    }

    /// Load synced content from device and auto-select synced items
    fn load_and_select_synced_content(&mut self, device: &Device) {
        if let Ok(Some(manifest)) = crate::device::SyncManifest::load(&device.mount_point) {
            // Load synced IDs
            self.synced_album_ids = manifest.synced_albums.iter().map(|a| a.id.clone()).collect();
            self.synced_playlist_ids = manifest.synced_playlists.iter().map(|p| p.id.clone()).collect();
            self.active_device = Some(device.clone());

            // Auto-select synced items
            self.selected_albums = self.synced_album_ids.clone();
            self.selected_playlists = self.synced_playlist_ids.clone();

            // Group synced albums by artist name
            let mut albums_by_artist: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();

            // Create Album objects from manifest data for albums not in cache
            for synced in &manifest.synced_albums {
                // Track album IDs per artist name
                albums_by_artist
                    .entry(synced.artist.clone())
                    .or_default()
                    .push(synced.id.clone());

                if !self.album_cache.contains_key(&synced.id) {
                    let album = Album {
                        id: synced.id.clone(),
                        name: synced.album.clone(),
                        artist: Some(synced.artist.clone()),
                        artist_id: None,
                        cover_art: None,
                        song_count: Some(synced.track_count),
                        duration: None,
                        year: None,
                        genre: None,
                    };
                    self.album_cache.insert(album.id.clone(), album);
                }
            }

            // Populate artist_album_ids by matching artist names to IDs
            for (artist_name, album_ids) in albums_by_artist {
                if let Some(artist) = self.artists.iter().find(|a| a.name == artist_name) {
                    self.artist_album_ids.insert(artist.id.clone(), album_ids);
                }
            }

            self.update_artist_selection_status();
        }
    }

    /// Toggle selection of all albums for an artist
    fn toggle_artist_selection(&mut self, artist_id: &str) {
        if let Some(album_ids) = self.artist_album_ids.get(artist_id) {
            let album_ids = album_ids.clone();
            let all_selected = album_ids.iter().all(|id| self.selected_albums.contains(id));

            if all_selected {
                // Deselect all albums for this artist
                for id in &album_ids {
                    self.selected_albums.remove(id);
                }
                self.selected_artists.remove(artist_id);
            } else {
                // Select all albums for this artist
                for id in &album_ids {
                    self.selected_albums.insert(id.clone());
                }
                self.selected_artists.insert(artist_id.to_string());
            }
        }
    }

    /// Check if an artist is fully selected (all albums selected)
    fn is_artist_selected(&self, artist_id: &str) -> bool {
        if let Some(album_ids) = self.artist_album_ids.get(artist_id) {
            !album_ids.is_empty() && album_ids.iter().all(|id| self.selected_albums.contains(id))
        } else {
            false
        }
    }

    /// Update artist selection status based on current album selections
    fn update_artist_selection_status(&mut self) {
        let artist_ids: Vec<String> = self.artist_album_ids.keys().cloned().collect();
        for artist_id in artist_ids {
            if self.is_artist_selected(&artist_id) {
                self.selected_artists.insert(artist_id);
            } else {
                self.selected_artists.remove(&artist_id);
            }
        }
    }

    /// Set status message with auto-clear timeout
    fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = message.into();
        self.status_message_time = Some(std::time::Instant::now());
    }

    /// Clear status message
    fn clear_status(&mut self) {
        self.status_message.clear();
        self.status_message_time = None;
    }

    /// Check and clear status message if timeout expired (3 seconds)
    fn check_status_timeout(&mut self) {
        if let Some(time) = self.status_message_time
            && time.elapsed() > std::time::Duration::from_secs(3)
        {
            self.clear_status();
        }
    }

    /// Apply search filter to current view
    fn apply_filter(&mut self) {
        let query = self.search_query.to_lowercase();
        if query.is_empty() {
            self.filtered_indices.clear();
            return;
        }

        self.filtered_indices = match &self.view {
            BrowseView::Artists => self
                .artists
                .iter()
                .enumerate()
                .filter(|(_, a)| a.name.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect(),
            BrowseView::Albums { .. } => self
                .albums
                .iter()
                .enumerate()
                .filter(|(_, a)| a.name.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect(),
            BrowseView::Playlists => self
                .playlists
                .iter()
                .enumerate()
                .filter(|(_, p)| p.name.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect(),
            _ => Vec::new(),
        };

        // Reset selection to first filtered item
        if !self.filtered_indices.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    /// Clear search filter
    fn clear_filter(&mut self) {
        self.search_mode = false;
        self.search_query.clear();
        self.filtered_indices.clear();
    }

    /// Get the actual index in the original list from display index
    fn get_actual_index(&self, display_idx: usize) -> usize {
        if self.filtered_indices.is_empty() {
            display_idx
        } else {
            self.filtered_indices.get(display_idx).copied().unwrap_or(display_idx)
        }
    }

    fn current_list_len(&self) -> usize {
        // If we have a filter active, use filtered count
        if !self.filtered_indices.is_empty() {
            return self.filtered_indices.len();
        }

        match &self.view {
            BrowseView::Artists => self.artists.len(),
            BrowseView::Albums { .. } => self.albums.len(),
            BrowseView::AlbumTracks { album } => album.song_count.unwrap_or(0) as usize,
            BrowseView::Playlists => self.playlists.len(),
            BrowseView::PlaylistTracks { playlist } => playlist.song_count.unwrap_or(0) as usize,
            BrowseView::DeviceSelection => self.mounted_devices.len() + self.unmounted_devices.len(),
            BrowseView::SyncProgress => self.sync_progress.log_messages.len(),
            BrowseView::SyncConfirmation => 2, // Yes/No options
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
    // Enable TUI mode to suppress stderr logging
    crate::utils::set_tui_mode(true);

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create state
    let mut state = BrowserState::new(initial_view.clone());

    // Try to detect connected device and load its sync manifest
    if let Ok(devices) = DeviceDetector::scan().await
        && let Some(device) = devices.first() {
            state.load_synced_content(device);
        }

    // Load initial data
    state.status_message = "Loading...".to_string();
    match &initial_view {
        BrowseView::Artists | BrowseView::Albums { .. } | BrowseView::AlbumTracks { .. } => {
            state.artists = client.get_artists().await?;
        }
        BrowseView::Playlists | BrowseView::PlaylistTracks { .. } => {
            state.playlists = client.get_playlists().await?;
        }
        BrowseView::DeviceSelection | BrowseView::SyncProgress | BrowseView::SyncConfirmation => {
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

    // Disable TUI mode to restore normal logging
    crate::utils::set_tui_mode(false);

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

        // Check for status message timeout
        state.check_status_timeout();

        // Draw UI
        terminal.draw(|f| draw_ui(f, state))?;

        // Handle input
        if event::poll(std::time::Duration::from_millis(50))?
            && let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                // Handle help overlay first
                if state.show_help {
                    // Any key closes help
                    state.show_help = false;
                    continue;
                }

                // Handle search mode input
                if state.search_mode {
                    match key.code {
                        KeyCode::Esc => {
                            state.clear_filter();
                        }
                        KeyCode::Enter => {
                            state.search_mode = false;
                        }
                        KeyCode::Backspace => {
                            state.search_query.pop();
                            state.apply_filter();
                        }
                        KeyCode::Char(c) => {
                            state.search_query.push(c);
                            state.apply_filter();
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') => {
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
                    KeyCode::Esc => {
                        // Esc clears filter if active, otherwise acts like backspace
                        if !state.search_query.is_empty() {
                            state.clear_filter();
                        } else if state.view == BrowseView::DeviceSelection {
                            state.view = BrowseView::Artists;
                            state.list_state.select(Some(0));
                        } else if state.view == BrowseView::SyncConfirmation {
                            // Cancel sync confirmation
                            state.sync_selection = None;
                            state.pending_deletions = None;
                            state.view = BrowseView::Artists;
                            state.list_state.select(Some(0));
                        } else if state.view != BrowseView::SyncProgress {
                            handle_back(state, client).await?;
                        }
                    }
                    KeyCode::Char('s') => {
                        // Start sync
                        if state.view != BrowseView::DeviceSelection && state.view != BrowseView::SyncProgress && state.view != BrowseView::SyncConfirmation {
                            let selection = build_selection(state, client).await?;
                            let deletions = calculate_deletions(state);

                            if selection.is_empty() && deletions.is_empty() {
                                if state.selected_albums.is_empty() && state.selected_playlists.is_empty() {
                                    state.set_status("No items selected!");
                                } else {
                                    state.set_status("All selected items already synced");
                                }
                            } else if state.selected_device.is_some() {
                                // Device already selected
                                if !deletions.is_empty() {
                                    // Show confirmation for deletions
                                    state.sync_selection = Some(selection);
                                    state.pending_deletions = Some(deletions);
                                    state.view = BrowseView::SyncConfirmation;
                                } else {
                                    // No deletions, start sync directly
                                    start_sync(state, client, selection, deletions).await?;
                                }
                            } else {
                                // No device selected yet
                                state.set_status("Select a device first with 'd'");
                            }
                        }
                    }
                    KeyCode::Char('d') => {
                        // Select device
                        if state.view != BrowseView::DeviceSelection && state.view != BrowseView::SyncProgress {
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
                            // Select device and load synced content
                            handle_device_select(state, client).await?;
                        } else if state.view == BrowseView::SyncConfirmation {
                            // Confirm sync with deletions
                            if let (Some(selection), Some(deletions)) = (state.sync_selection.take(), state.pending_deletions.take()) {
                                start_sync(state, client, selection, deletions).await?;
                            }
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
                            handle_toggle(state, client, terminal).await?;
                        }
                    }
                    KeyCode::Char('a') => {
                        if state.view != BrowseView::SyncProgress {
                            handle_select_all(state);
                        }
                    }
                    KeyCode::Char('A') => {
                        // Deselect all in current view
                        if state.view != BrowseView::SyncProgress {
                            handle_deselect_all(state);
                        }
                    }
                    KeyCode::Char('/') => {
                        // Enter search mode
                        if state.view != BrowseView::DeviceSelection && state.view != BrowseView::SyncProgress {
                            state.search_mode = true;
                            state.search_query.clear();
                        }
                    }
                    KeyCode::Char('?') => {
                        // Toggle help overlay
                        state.show_help = !state.show_help;
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
        SyncProgressEvent::Complete { albums_synced, playlists_synced, tracks_downloaded, bytes_downloaded, albums_deleted, playlists_deleted } => {
            state.sync_progress.is_complete = true;
            state.sync_progress.bytes_downloaded = bytes_downloaded;
            let mb = bytes_downloaded as f64 / 1_048_576.0;
            let delete_info = if albums_deleted > 0 || playlists_deleted > 0 {
                format!(", deleted {} albums, {} playlists", albums_deleted, playlists_deleted)
            } else {
                String::new()
            };
            state.sync_progress.log_messages.push(format!(
                "Sync complete! {} albums, {} playlists, {} tracks ({:.1} MB){}",
                albums_synced, playlists_synced, tracks_downloaded, mb, delete_info
            ));
        }
        SyncProgressEvent::DeletionStarted { albums_to_delete, playlists_to_delete } => {
            state.sync_progress.log_messages.push(format!(
                "Deleting {} albums, {} playlists...",
                albums_to_delete, playlists_to_delete
            ));
        }
        SyncProgressEvent::AlbumDeleted { artist, album } => {
            state.sync_progress.log_messages.push(format!(
                "  Deleted: {} - {}", artist, album
            ));
        }
        SyncProgressEvent::AlbumDeleteFailed { artist, album, error } => {
            state.sync_progress.log_messages.push(format!(
                "  DELETE FAILED: {} - {} ({})", artist, album, error
            ));
        }
        SyncProgressEvent::PlaylistDeleted { name } => {
            state.sync_progress.log_messages.push(format!(
                "  Deleted playlist: {}", name
            ));
        }
        SyncProgressEvent::PlaylistDeleteFailed { name, error } => {
            state.sync_progress.log_messages.push(format!(
                "  DELETE FAILED: {} ({})", name, error
            ));
        }
    }
}

/// Start sync with the selected device
async fn start_sync(state: &mut BrowserState, client: &SubsonicClient, selection: SyncSelection, deletions: DeletionSelection) -> Result<()> {
    let Some(ref device) = state.selected_device else {
        state.status_message = "No device selected!".to_string();
        return Ok(());
    };

    // Create progress channel
    let (tx, rx) = mpsc::channel::<SyncProgressEvent>(100);

    // Store state for sync
    state.sync_selection = Some(selection.clone());
    state.pending_deletions = Some(deletions.clone());
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

        if let Err(e) = engine.sync_with_progress(&selection, &deletions, tx.clone()).await {
            let _ = tx.send(SyncProgressEvent::Error {
                message: format!("Sync failed: {}", e),
            }).await;
        }
    });

    // Switch to sync progress view
    state.view = BrowseView::SyncProgress;
    state.status_message.clear();

    Ok(())
}

/// Handle device selection - loads synced content and returns to browse
async fn handle_device_select(state: &mut BrowserState, _client: &SubsonicClient) -> Result<bool> {
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

    // Load synced content and auto-select
    state.load_and_select_synced_content(&device);
    state.selected_device = Some(device.clone());

    // Count synced items
    let album_count = state.selected_albums.len();
    let playlist_count = state.selected_playlists.len();

    // Return to Artists view
    state.view = BrowseView::Artists;
    state.list_state.select(Some(0));
    state.set_status(format!(
        "Device: {} - {} albums, {} playlists synced",
        device.display_name(),
        album_count,
        playlist_count
    ));

    Ok(true)
}

async fn handle_enter(state: &mut BrowserState, client: &SubsonicClient) -> Result<()> {
    let display_idx = state.list_state.selected().unwrap_or(0);
    let actual_idx = state.get_actual_index(display_idx);

    match &state.view {
        BrowseView::Artists => {
            if let Some(artist) = state.artists.get(actual_idx) {
                state.status_message = format!("Loading albums for {}...", artist.name);
                let artist_details = client.get_artist(&artist.id).await?;

                // Cache album IDs for this artist (for artist-level selection)
                let album_ids: Vec<String> = artist_details.album.iter().map(|a| a.id.clone()).collect();
                state.artist_album_ids.insert(artist.id.clone(), album_ids);

                state.albums = artist_details.album;
                // Populate album cache for selection building
                for album in &state.albums {
                    state.album_cache.insert(album.id.clone(), album.clone());
                }
                state.view = BrowseView::Albums {
                    artist_id: artist.id.clone(),
                    artist_name: artist.name.clone(),
                };
                state.clear_filter(); // Clear filter when navigating
                state.list_state.select(Some(0));
                state.status_message.clear();
            }
        }
        BrowseView::Albums { .. } => {
            if let Some(album) = state.albums.get(actual_idx) {
                state.view = BrowseView::AlbumTracks {
                    album: album.clone(),
                };
                state.clear_filter();
                state.list_state.select(Some(0));
            }
        }
        BrowseView::Playlists => {
            if let Some(playlist) = state.playlists.get(actual_idx) {
                state.view = BrowseView::PlaylistTracks {
                    playlist: playlist.clone(),
                };
                state.clear_filter();
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

async fn handle_toggle(
    state: &mut BrowserState,
    client: &SubsonicClient,
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<()> {
    let display_idx = state.list_state.selected().unwrap_or(0);
    let actual_idx = state.get_actual_index(display_idx);

    match &state.view {
        BrowseView::Artists => {
            // Toggle all albums for this artist
            if let Some(artist) = state.artists.get(actual_idx) {
                let artist_id = artist.id.clone();
                let artist_name = artist.name.clone();

                // If we haven't fetched this artist's albums yet, fetch them now
                if !state.artist_album_ids.contains_key(&artist_id) {
                    state.status_message = format!("Loading {}...", artist_name);
                    terminal.draw(|f| draw_ui(f, state))?;

                    let artist_details = client.get_artist(&artist_id).await?;
                    let album_ids: Vec<String> = artist_details.album.iter().map(|a| a.id.clone()).collect();
                    state.artist_album_ids.insert(artist_id.clone(), album_ids);
                    // Cache album objects for selection building
                    for album in artist_details.album {
                        state.album_cache.insert(album.id.clone(), album);
                    }
                    state.status_message.clear();
                }

                state.toggle_artist_selection(&artist_id);
            }
        }
        BrowseView::Albums { .. } => {
            if let Some(album) = state.albums.get(actual_idx) {
                if state.selected_albums.contains(&album.id) {
                    state.selected_albums.remove(&album.id);
                } else {
                    state.selected_albums.insert(album.id.clone());
                }
                // Update artist selection status
                state.update_artist_selection_status();
            }
        }
        BrowseView::Playlists => {
            if let Some(playlist) = state.playlists.get(actual_idx) {
                if state.selected_playlists.contains(&playlist.id) {
                    state.selected_playlists.remove(&playlist.id);
                } else {
                    state.selected_playlists.insert(playlist.id.clone());
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_select_all(state: &mut BrowserState) {
    match &state.view {
        BrowseView::Albums { .. } => {
            for album in &state.albums {
                state.selected_albums.insert(album.id.clone());
            }
            state.update_artist_selection_status();
        }
        BrowseView::Playlists => {
            for playlist in &state.playlists {
                state.selected_playlists.insert(playlist.id.clone());
            }
        }
        BrowseView::Artists => {
            // Select all visited artists
            for artist_id in state.artist_album_ids.keys() {
                if let Some(album_ids) = state.artist_album_ids.get(artist_id).cloned() {
                    for album_id in album_ids {
                        state.selected_albums.insert(album_id);
                    }
                    state.selected_artists.insert(artist_id.clone());
                }
            }
        }
        _ => {}
    }
}

fn handle_deselect_all(state: &mut BrowserState) {
    match &state.view {
        BrowseView::Albums { .. } => {
            for album in &state.albums {
                state.selected_albums.remove(&album.id);
            }
            state.update_artist_selection_status();
        }
        BrowseView::Playlists => {
            for playlist in &state.playlists {
                state.selected_playlists.remove(&playlist.id);
            }
        }
        BrowseView::Artists => {
            // Deselect all albums and artists
            state.selected_albums.clear();
            state.selected_artists.clear();
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
        BrowseView::DeviceSelection | BrowseView::SyncProgress | BrowseView::SyncConfirmation => {
            // Don't switch views from device selection, sync progress, or confirmation
        }
    }
    Ok(())
}

async fn build_selection(state: &BrowserState, _client: &SubsonicClient) -> Result<SyncSelection> {
    let mut selection = SyncSelection::new();

    // Add selected albums that are NOT already synced
    for album_id in &state.selected_albums {
        if !state.synced_album_ids.contains(album_id)
            && let Some(album) = state.album_cache.get(album_id)
        {
            selection.albums.push(album.clone());
        }
    }

    // Add selected playlists that are NOT already synced
    for playlist_id in &state.selected_playlists {
        if !state.synced_playlist_ids.contains(playlist_id)
            && let Some(playlist) = state.playlists.iter().find(|p| &p.id == playlist_id)
        {
            selection.playlists.push(playlist.clone());
        }
    }

    Ok(selection)
}

/// Calculate items to delete (synced but no longer selected)
fn calculate_deletions(state: &BrowserState) -> DeletionSelection {
    let mut deletions = DeletionSelection::default();

    // Find albums to delete: synced but not selected
    if let Some(device) = &state.active_device
        && let Ok(Some(manifest)) = SyncManifest::load(&device.mount_point)
    {
        for album_id in &state.synced_album_ids {
            if !state.selected_albums.contains(album_id)
                && let Some(synced) = manifest.synced_albums.iter().find(|a| &a.id == album_id)
            {
                deletions.albums.push((
                    album_id.clone(),
                    synced.artist.clone(),
                    synced.album.clone(),
                ));
            }
        }

        // Find playlists to delete: synced but not selected
        for playlist_id in &state.synced_playlist_ids {
            if !state.selected_playlists.contains(playlist_id)
                && let Some(synced) = manifest.synced_playlists.iter().find(|p| &p.id == playlist_id)
            {
                deletions.playlists.push((
                    playlist_id.clone(),
                    synced.name.clone(),
                ));
            }
        }
    }

    deletions
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

/// Draw the sync confirmation view
fn draw_sync_confirmation(f: &mut Frame, state: &BrowserState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(10),    // Content
            Constraint::Length(3),  // Footer
        ])
        .split(f.area());

    let header = Paragraph::new("Sync Confirmation")
        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, chunks[0]);

    let mut lines = vec![];

    if let Some(ref deletions) = state.pending_deletions
        && (!deletions.albums.is_empty() || !deletions.playlists.is_empty())
    {
        lines.push(Line::styled("Will DELETE:", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)));
        for (_, artist, album) in &deletions.albums {
            lines.push(Line::styled(format!("  - {} - {}", artist, album), Style::default().fg(Color::Red)));
        }
        for (_, name) in &deletions.playlists {
            lines.push(Line::styled(format!("  - Playlist: {}", name), Style::default().fg(Color::Red)));
        }
        lines.push(Line::from(""));
    }

    if let Some(ref selection) = state.sync_selection
        && (!selection.albums.is_empty() || !selection.playlists.is_empty())
    {
        lines.push(Line::styled("Will ADD:", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
        for album in &selection.albums {
            let artist = album.artist.as_deref().unwrap_or("Unknown");
            lines.push(Line::styled(format!("  + {} - {}", artist, album.name), Style::default().fg(Color::Green)));
        }
        for playlist in &selection.playlists {
            lines.push(Line::styled(format!("  + Playlist: {}", playlist.name), Style::default().fg(Color::Green)));
        }
    }

    let content = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    f.render_widget(content, chunks[1]);

    let footer = Paragraph::new("Press Enter to confirm, Esc to cancel")
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer, chunks[2]);
}

fn draw_ui(f: &mut Frame, state: &BrowserState) {
    // Special layout for sync progress view
    if state.view == BrowseView::SyncProgress {
        draw_sync_progress(f, state);
        return;
    }

    // Special layout for sync confirmation view
    if state.view == BrowseView::SyncConfirmation {
        draw_sync_confirmation(f, state);
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
        BrowseView::SyncConfirmation => "Confirm Sync",
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

    // Build the list of indices to display (either filtered or all)
    let artist_indices: Vec<usize> = if !state.filtered_indices.is_empty() {
        state.filtered_indices.clone()
    } else {
        (0..state.artists.len()).collect()
    };

    let album_indices: Vec<usize> = if !state.filtered_indices.is_empty() {
        state.filtered_indices.clone()
    } else {
        (0..state.albums.len()).collect()
    };

    let playlist_indices: Vec<usize> = if !state.filtered_indices.is_empty() {
        state.filtered_indices.clone()
    } else {
        (0..state.playlists.len()).collect()
    };

    // List
    let items: Vec<ListItem> = match &state.view {
        BrowseView::Artists => artist_indices
            .iter()
            .filter_map(|&i| state.artists.get(i))
            .map(|a| {
                let album_count = a.album_count.map(|c| format!(" ({} albums)", c)).unwrap_or_default();

                // Check if artist is fully or partially selected
                let (prefix, style) = if let Some(album_ids) = state.artist_album_ids.get(&a.id) {
                    let selected_count = album_ids.iter().filter(|id| state.selected_albums.contains(*id)).count();
                    if selected_count == album_ids.len() && !album_ids.is_empty() {
                        // All albums selected
                        ("[x] ", Style::default().fg(Color::Green))
                    } else if selected_count > 0 {
                        // Some albums selected
                        ("[-] ", Style::default().fg(Color::Yellow))
                    } else {
                        // No albums selected
                        ("[ ] ", Style::default())
                    }
                } else {
                    // Haven't visited this artist yet, no checkbox
                    ("    ", Style::default())
                };

                ListItem::new(format!("{}{}{}", prefix, a.name, album_count)).style(style)
            })
            .collect(),
        BrowseView::Albums { .. } => album_indices
            .iter()
            .filter_map(|&i| state.albums.get(i))
            .map(|a| {
                let selected = state.selected_albums.contains(&a.id);
                let synced = state.synced_album_ids.contains(&a.id);
                let prefix = if selected { "[x] " } else { "[ ] " };
                let suffix = if synced { " [SYNCED]" } else { "" };
                let year = a.year.map(|y| format!(" ({})", y)).unwrap_or_default();
                let style = if selected {
                    Style::default().fg(Color::Green)
                } else if synced {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{}{}{}{}", prefix, a.name, year, suffix)).style(style)
            })
            .collect(),
        BrowseView::AlbumTracks { album } => {
            vec![ListItem::new(format!(
                "Album has {} tracks - press Backspace to go back",
                album.song_count.unwrap_or(0)
            ))]
        }
        BrowseView::Playlists => playlist_indices
            .iter()
            .filter_map(|&i| state.playlists.get(i))
            .map(|p| {
                let selected = state.selected_playlists.contains(&p.id);
                let synced = state.synced_playlist_ids.contains(&p.id);
                let prefix = if selected { "[x] " } else { "[ ] " };
                let suffix = if synced { " [SYNCED]" } else { "" };
                let count = p.song_count.map(|c| format!(" ({} tracks)", c)).unwrap_or_default();
                let style = if selected {
                    Style::default().fg(Color::Green)
                } else if synced {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{}{}{}{}", prefix, p.name, count, suffix)).style(style)
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
        BrowseView::SyncConfirmation => {
            // This case should never be reached due to separate draw function, but compiler needs it
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

    // Footer/help with device info
    let device_info = if let Some(ref device) = state.active_device {
        let name = device.display_name();
        let free_gb = device.free_space as f64 / 1_073_741_824.0;
        format!(" | Device: {} ({:.1} GB free)", name, free_gb)
    } else {
        String::new()
    };

    let help_text = match &state.view {
        BrowseView::Artists => format!("↑/↓: Navigate | Space: Select | /: Search | ?: Help | d: Device | s: Sync | q: Done{}", device_info),
        BrowseView::Albums { .. } => format!("↑/↓: Navigate | Space: Select | a/A: All/None | /: Search | d: Device | s: Sync | q: Done{}", device_info),
        BrowseView::Playlists => format!("↑/↓: Navigate | Space: Select | a/A: All/None | /: Search | d: Device | s: Sync | q: Done{}", device_info),
        BrowseView::DeviceSelection => "↑/↓: Navigate | Enter: Select device | Backspace/q: Cancel".to_string(),
        _ => "Backspace: Back | q: Done".to_string(),
    };

    let footer = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer, chunks[2]);

    // Search input overlay
    if state.search_mode || !state.search_query.is_empty() {
        let search_text = if state.search_mode {
            format!("Search: {}█", state.search_query)
        } else {
            format!("Filter: {} (Esc to clear)", state.search_query)
        };
        let search_style = if state.search_mode {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::Cyan)
        };
        let search = Paragraph::new(search_text)
            .style(search_style)
            .block(Block::default().borders(Borders::ALL).title("Search"));
        let area = centered_rect(60, 3, f.area());
        f.render_widget(search, area);
    }

    // Help overlay
    if state.show_help {
        let help_lines = vec![
            Line::from("Keyboard Shortcuts"),
            Line::from(""),
            Line::styled("Navigation", Style::default().add_modifier(Modifier::BOLD)),
            Line::from("  ↑/k, ↓/j    Move up/down"),
            Line::from("  Enter/l     Enter/expand"),
            Line::from("  Backspace/h Go back"),
            Line::from("  Tab         Switch Artists/Playlists"),
            Line::from(""),
            Line::styled("Selection", Style::default().add_modifier(Modifier::BOLD)),
            Line::from("  Space       Toggle selection"),
            Line::from("  a           Select all in view"),
            Line::from("  A           Deselect all in view"),
            Line::from(""),
            Line::styled("Search & Actions", Style::default().add_modifier(Modifier::BOLD)),
            Line::from("  /           Search/filter"),
            Line::from("  d           Select device"),
            Line::from("  s           Start sync"),
            Line::from("  q, Esc      Quit/Cancel"),
            Line::from(""),
            Line::styled("Press any key to close", Style::default().fg(Color::DarkGray)),
        ];
        let help_popup = Paragraph::new(help_lines)
            .block(Block::default()
                .borders(Borders::ALL)
                .title("Help")
                .style(Style::default().bg(Color::Black)));
        let area = centered_rect(50, 22, f.area());
        f.render_widget(ratatui::widgets::Clear, area);
        f.render_widget(help_popup, area);
    }

    // Status message overlay
    if !state.status_message.is_empty() && !state.show_help {
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
