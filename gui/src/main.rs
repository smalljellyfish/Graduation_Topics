// 本地模組
mod osu;
mod osuhelper;
mod spotify;

// 標準庫導入
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::default::Default;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use std::time::{Duration, Instant};

// 第三方庫導入
use anyhow::{anyhow, Context, Result};
use backoff::backoff::Backoff;
use backoff::exponential::ExponentialBackoff;
use backoff::SystemClock;
use chrono::{DateTime, TimeDelta, Utc};
use clipboard::{ClipboardContext, ClipboardProvider};
use eframe::{self, egui};
use egui::{
    FontData, FontDefinitions, FontFamily, TextureHandle, TextureWrapMode, ViewportBuilder,
};

use log::{debug, error, info, LevelFilter};
use parking_lot::Mutex as ParkingLotMutex;
use reqwest::Client;
use rodio::{OutputStream, OutputStreamHandle, Sink};
use rspotify::{
    clients::{BaseClient, OAuthClient},
    model::{FullTrack, PlaylistId, SimplifiedPlaylist, TrackId},
    prelude::Id,
    scopes, AuthCodeSpotify, Credentials, OAuth, Token,
};
use serde::{Deserialize, Serialize};
use simplelog::*;
use thiserror::Error;
use tokio::{
    self,
    net::TcpListener,
    sync::{
        mpsc,
        mpsc::{Receiver, Sender},
        Mutex as TokioMutex, RwLock, Semaphore,
    },
    task::JoinHandle,
};

// 本地模組導入
use crate::osu::{
    delete_beatmap, get_beatmapset_by_id, get_beatmapset_details, get_beatmapsets,
    get_downloaded_beatmaps, get_osu_token, load_osu_covers, parse_osu_url, preview_beatmap,
    print_beatmap_info_gui, Beatmapset,
};
use crate::spotify::{
    add_track_to_liked, authorize_spotify, get_access_token, get_playlist_tracks, get_track_info,
    get_user_playlists, is_valid_spotify_url, load_spotify_icon, open_spotify_url,
    remove_track_from_liked, search_track, update_currently_playing_wrapper, Album, AuthStatus,
    CurrentlyPlaying, Image, SpotifyError, SpotifyUrlStatus, Track, TrackWithCover,
};
use lib::{
    check_and_refresh_token, get_app_data_path, load_background_path, load_download_directory,
    load_scale_factor, need_select_download_directory, read_config, read_login_info,
    save_background_path, save_download_directory, save_scale_factor, set_log_level, ConfigError,
};

use osuhelper::OsuHelper;

const BASE_SIDE_MENU_WIDTH: f32 = 300.0;
const MIN_SIDE_MENU_WIDTH: f32 = 200.0;
const MAX_SIDE_MENU_WIDTH: f32 = 500.0;
const BUTTON_SIZE: f32 = 40.0;
const ANIMATION_SPEED: f32 = 4.0;
const SEARCH_BAR_WIDTH_RATIO: f32 = 0.6;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("配置錯誤: {0}")]
    ConfigError(#[from] ConfigError),
    #[error("Spotify 錯誤: {0}")]
    SpotifyError(#[from] spotify::SpotifyError),
    #[error("Osu 錯誤: {0}")]
    OsuError(#[from] osu::OsuError),
    #[error("IO 錯誤: {0}")]
    IoError(#[from] std::io::Error),
    #[error(transparent)]
    AnyhowError(#[from] anyhow::Error),
    #[error("其他錯誤: {0}")]
    Other(String),
}

// 定義 AuthPlatform 列舉，用於標識不同的授權平台
#[derive(Eq, PartialEq, Hash, Debug, Clone)]
pub enum AuthPlatform {
    Spotify,
    Osu,
}
// 定義 DownloadStatus 列舉，用於標識不同的下載狀態
#[derive(Clone, Copy, PartialEq)]
pub enum DownloadStatus {
    NotStarted,
    Waiting,
    Downloading,
    Completed,
}
// 定義 PlaylistCache 結構，用於緩存播放列表曲目
#[derive(Serialize, Deserialize)]
struct PlaylistCache {
    tracks: Vec<FullTrack>,
    last_updated: SystemTime,
}

// 定義 AuthManager 結構，儲存授權狀態和錯誤記錄
pub struct AuthManager {
    status: ParkingLotMutex<HashMap<AuthPlatform, AuthStatus>>,
    error_logged: AtomicBool,
}

impl AuthManager {
    pub fn new() -> Self {
        let mut status = HashMap::new();
        status.insert(AuthPlatform::Spotify, AuthStatus::NotStarted);
        Self {
            status: ParkingLotMutex::new(status),
            error_logged: AtomicBool::new(false),
        }
    }

    pub fn reset(&self, platform: &AuthPlatform) {
        self.status
            .lock()
            .insert(platform.clone(), AuthStatus::NotStarted);
        self.error_logged.store(false, Ordering::Relaxed);
    }

    pub fn update_status(&self, platform: &AuthPlatform, new_status: AuthStatus) {
        let mut status = self.status.lock();
        let old_status = status
            .get(platform)
            .cloned()
            .unwrap_or(AuthStatus::NotStarted);
        status.entry(platform.clone()).or_insert(new_status.clone());

        if let AuthStatus::Failed(ref error) = new_status {
            if !matches!(old_status, AuthStatus::Failed(_)) {
                error!("{:?} 授權失敗: {}", platform, error);
            }
        }
    }

    pub fn get_status(&self, platform: &AuthPlatform) -> AuthStatus {
        self.status
            .lock()
            .get(platform)
            .cloned()
            .unwrap_or(AuthStatus::NotStarted)
    }

    pub fn get_all_statuses(&self) -> HashMap<AuthPlatform, AuthStatus> {
        self.status.lock().clone()
    }
}

// 定義 SpotifySearchApp結構，儲存程式狀態和數據
struct SearchApp {
    // 認證相關
    access_token: Arc<tokio::sync::Mutex<String>>,
    auth_in_progress: Arc<AtomicBool>,
    auth_manager: Arc<AuthManager>,
    auth_start_time: Option<Instant>,
    spotify_authorized: Arc<AtomicBool>,
    spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,

    // 使用者資訊
    spotify_user_avatar: Arc<Mutex<Option<egui::TextureHandle>>>,
    spotify_user_avatar_url: Arc<Mutex<Option<String>>>,
    spotify_user_name: Arc<Mutex<Option<String>>>,

    // 搜索相關
    search_query: String,
    is_searching: Arc<AtomicBool>,
    search_results: Arc<tokio::sync::Mutex<Vec<Track>>>,
    osu_search_results: Arc<tokio::sync::Mutex<Vec<Beatmapset>>>,
    displayed_spotify_results: usize,
    displayed_osu_results: usize,
    downloaded_maps_search: String,
    playlist_search_query: String,
    tracks_search_query: String,

    // 播放列表和曲目
    spotify_user_playlists: Arc<Mutex<Vec<SimplifiedPlaylist>>>,
    spotify_playlist_tracks: Arc<Mutex<Vec<FullTrack>>>,
    spotify_liked_tracks: Arc<Mutex<Vec<FullTrack>>>,
    selected_playlist: Option<SimplifiedPlaylist>,
    currently_playing: Arc<Mutex<Option<CurrentlyPlaying>>>,

    // UI 狀態
    show_auth_progress: bool,
    show_side_menu: bool,
    side_menu_width: Option<f32>,
    show_spotify_now_playing: bool,
    show_playlists: bool,
    show_liked_tracks: bool,
    spotify_scroll_to_top: bool,
    osu_scroll_to_top: bool,
    global_font_size: f32,
    search_bar_expanded: bool,
    is_beatmap_playing: bool,
    scale_factor: f32,
    is_first_update: bool,
    show_downloaded_maps: bool,
    expanded_map_indices: HashSet<String>,
    show_osu_search_bar: bool,
    show_playlist_search_bar: bool,
    show_tracks_search_bar: bool,


    // 紋理和圖像
    avatar_load_handle: Option<tokio::task::JoinHandle<()>>,
    cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>>,
    playlist_cover_textures: Arc<Mutex<HashMap<String, Option<TextureHandle>>>>,
    default_avatar_texture: Option<egui::TextureHandle>,
    spotify_icon: Option<egui::TextureHandle>,
    texture_cache: Arc<RwLock<HashMap<String, Arc<TextureHandle>>>>,
    preloaded_icons: HashMap<String, egui::TextureHandle>,

    // 網絡和客戶端
    client: Arc<tokio::sync::Mutex<Client>>,
    listener: Arc<TokioMutex<Option<TcpListener>>>,

    // 錯誤處理
    err_msg: Arc<tokio::sync::Mutex<String>>,
    error_message: Arc<tokio::sync::Mutex<String>>,
    config_errors: Arc<Mutex<Vec<String>>>,

    // 狀態管理
    initialized: bool,
    need_reload_avatar: Arc<AtomicBool>,
    need_repaint: Arc<AtomicBool>,
    last_update: Arc<Mutex<Option<Instant>>>,
    last_avatar_update: DateTime<Utc>,
    beatmapset_download_statuses: Arc<Mutex<HashMap<i32, DownloadStatus>>>,

    // 異步通信
    receiver: Option<tokio::sync::mpsc::Receiver<(usize, Arc<TextureHandle>, (f32, f32))>>,
    sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,

    // UI 元素狀態
    side_menu_animation: HashMap<egui::Id, f32>,
    global_volume: f32,
    expanded_track_index: Option<usize>,
    expanded_beatmapset_index: Option<usize>,

    // 其他功能
    debug_mode: bool,
    ctx: egui::Context,
    selected_beatmapset: Option<usize>,
    should_detect_now_playing: Arc<AtomicBool>,
    spotify_track_liked_status: Arc<Mutex<HashMap<String, bool>>>,
    osu_download_statuses: HashMap<usize, DownloadStatus>,
    osu_helper: OsuHelper,

    // 快取
    liked_songs_cache: Arc<Mutex<Option<PlaylistCache>>>,
    cache_ttl: Duration,
    texture_load_queue: Arc<Mutex<BinaryHeap<Reverse<(usize, String)>>>>,

    // 更新檢查
    update_check_result: Arc<Mutex<Option<bool>>>,
    update_check_sender: Sender<bool>,
    update_check_receiver: Receiver<bool>,
    last_background_key: String,

    // 下載相關
    download_directory: PathBuf,
    status_sender: tokio::sync::mpsc::Sender<(i32, DownloadStatus)>,
    status_receiver: tokio::sync::mpsc::Receiver<(i32, DownloadStatus)>,
    download_queue_sender: mpsc::Sender<i32>,
    download_queue_receiver: Arc<Mutex<Option<mpsc::Receiver<i32>>>>,
    download_semaphore: Arc<Semaphore>,
    current_downloads: Arc<AtomicUsize>,

    // 預覽播放
    audio_output: Option<(OutputStream, OutputStreamHandle)>,
    current_previews: Arc<TokioMutex<HashMap<i32, Sink>>>,

    // 自定義背景
    custom_background_path: Option<PathBuf>,
    custom_background: Option<egui::TextureHandle>,
    need_load_background: bool,
}

impl eframe::App for SearchApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.initialized {
            self.initialize(ctx);
        }
        if self.need_load_background {
            self.load_background(ctx);
            self.need_load_background = false;
        }
        if self.is_first_update {
            ctx.set_pixels_per_point(self.scale_factor);
            self.is_first_update = false;
        }

        self.handle_avatar_loading(ctx);
        self.check_auth_status();
        self.handle_config_errors(ctx);
        self.update_ui(ctx);
        self.handle_debug_mode();
        self.update_current_playing(ctx);
        self.handle_download_status_updates();
        self.check_and_update_avatar(ctx);

        ctx.request_repaint();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.clean_up_resources();
    }
}

impl SearchApp {
    fn initialize(&mut self, ctx: &egui::Context) {
        self.spawn_osu_cover_loader(ctx);
        self.spawn_texture_receiver();
        self.spawn_access_token_fetcher();
        self.spawn_error_message_handler(ctx);
        self.initialized = true;
    }

    fn spawn_osu_cover_loader(&self, ctx: &egui::Context) {
        let sender = self.sender.clone();
        let ctx = ctx.clone();
        let debug_mode = self.debug_mode;

        tokio::spawn(async move {
            if let Err(e) = load_osu_covers(vec![], ctx.clone(), sender).await {
                Self::handle_osu_cover_load_error(e, debug_mode, &ctx);
            }
        });
    }

    fn load_background(&mut self, ctx: &egui::Context) {
        match load_background_path() {
            Ok(Some(path)) => {
                self.custom_background_path = Some(path.clone());
                if let Err(e) = self.load_custom_background(ctx) {
                    error!("加載自定義背景失敗: {:?}", e);
                    self.custom_background_path = None;
                }
            }
            Ok(None) => {
                // 沒有保存的背景路徑，使用默認背景
            }
            Err(e) => {
                error!("加載背景路徑失敗: {:?}", e);
            }
        }
    }

    fn handle_osu_cover_load_error(e: impl std::fmt::Debug, debug_mode: bool, ctx: &egui::Context) {
        error!("初始化時載入 osu 封面發生錯誤: {:?}", e);
        if debug_mode {
            ctx.request_repaint();
            egui::Window::new("錯誤").show(ctx, |ui| {
                ui.label(format!("載入 osu 封面錯誤: {:?}", e));
            });
        }
    }

    fn spawn_texture_receiver(&mut self) {
        let receiver = self.receiver.take().expect("Receiver already taken");
        let cover_textures = Arc::downgrade(&self.cover_textures);
        let need_repaint = Arc::downgrade(&self.need_repaint);

        tokio::spawn(async move {
            Self::process_texture_updates(receiver, cover_textures, need_repaint).await;
        });
    }

    async fn process_texture_updates(
        mut receiver: tokio::sync::mpsc::Receiver<(usize, Arc<TextureHandle>, (f32, f32))>,
        cover_textures: std::sync::Weak<
            RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>,
        >,
        need_repaint: std::sync::Weak<AtomicBool>,
    ) {
        while let Some((id, texture, dimensions)) = receiver.recv().await {
            if let (Some(cover_textures), Some(need_repaint)) =
                (cover_textures.upgrade(), need_repaint.upgrade())
            {
                let mut textures = cover_textures.write().await;
                textures.insert(id, Some((texture, dimensions)));

                // 實現緩存淘汰策略
                if textures.len() > 1000 {
                    // 設置最大容量限制
                    let oldest_id = *textures.keys().next().unwrap();
                    textures.remove(&oldest_id);
                }

                need_repaint.store(true, Ordering::SeqCst);
            } else {
                break;
            }
        }
    }

    fn spawn_access_token_fetcher(&self) {
        let access_token = Arc::downgrade(&self.access_token);
        let error_message = Arc::downgrade(&self.error_message);
        let client = Arc::downgrade(&self.client);
        let debug_mode = self.debug_mode;
        let is_searching = Arc::downgrade(&self.is_searching);
        let need_repaint = Arc::downgrade(&self.need_repaint);

        tokio::spawn(async move {
            if let (
                Some(access_token),
                Some(error_message),
                Some(client),
                Some(is_searching),
                Some(need_repaint),
            ) = (
                access_token.upgrade(),
                error_message.upgrade(),
                client.upgrade(),
                is_searching.upgrade(),
                need_repaint.upgrade(),
            ) {
                Self::fetch_access_token(
                    access_token,
                    error_message,
                    client,
                    debug_mode,
                    is_searching,
                    need_repaint,
                )
                .await;
            }
        });
    }

    async fn fetch_access_token(
        access_token: Arc<tokio::sync::Mutex<String>>,
        error_message: Arc<tokio::sync::Mutex<String>>,
        client: Arc<tokio::sync::Mutex<Client>>,
        debug_mode: bool,
        is_searching: Arc<AtomicBool>,
        need_repaint: Arc<AtomicBool>,
    ) {
        let client_guard = client.lock().await;
        match get_access_token(&*client_guard, debug_mode).await {
            Ok(token) => {
                let mut token_guard = access_token.lock().await;
                *token_guard = token;
            }
            Err(e) => Self::handle_access_token_error(e, error_message, is_searching, need_repaint),
        }
    }

    fn handle_access_token_error(
        e: impl std::fmt::Debug,
        error_message: Arc<tokio::sync::Mutex<String>>,
        is_searching: Arc<AtomicBool>,
        need_repaint: Arc<AtomicBool>,
    ) {
        let mut error = error_message.blocking_lock();
        *error = "Spotify 錯誤：無法獲取 token".to_string();
        error!("獲取 Spotify token 錯誤: {:?}", e);
        is_searching.store(false, Ordering::SeqCst);
        need_repaint.store(true, Ordering::SeqCst);
    }

    fn spawn_error_message_handler(&self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        let err_msg = Arc::downgrade(&self.err_msg);
        tokio::spawn(async move {
            if let Some(err_msg) = err_msg.upgrade() {
                Self::handle_error_messages(ctx, err_msg).await;
            }
        });
    }

    async fn handle_error_messages(ctx: egui::Context, err_msg: Arc<tokio::sync::Mutex<String>>) {
        let err_msg = err_msg.lock().await;
        if !err_msg.is_empty() {
            ctx.request_repaint();
            egui::Window::new("錯誤").show(&ctx, |ui| {
                ui.label(&err_msg.to_string());
            });
        }
    }

    fn handle_avatar_loading(&mut self, ctx: &egui::Context) {
        if self.need_reload_avatar() {
            self.start_load_spotify_avatar(ctx);
        }
    }

    fn need_reload_avatar(&self) -> bool {
        self.spotify_user_avatar.lock().unwrap().is_none()
            && self.spotify_user_avatar_url.lock().unwrap().is_some()
            && self.need_reload_avatar.load(Ordering::SeqCst)
    }

    fn start_load_spotify_avatar(&mut self, ctx: &egui::Context) {
        info!("觸發加載 Spotify 用戶頭像");
        let url = self
            .spotify_user_avatar_url
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        let ctx = ctx.clone();
        let need_reload_avatar = Arc::downgrade(&self.need_reload_avatar);
        let spotify_user_avatar = Arc::downgrade(&self.spotify_user_avatar);

        if let Some(handle) = self.avatar_load_handle.take() {
            handle.abort();
        }

        self.avatar_load_handle = Some(tokio::spawn(async move {
            if let (Some(need_reload_avatar), Some(spotify_user_avatar)) =
                (need_reload_avatar.upgrade(), spotify_user_avatar.upgrade())
            {
                Self::load_and_handle_avatar(url, ctx, need_reload_avatar, spotify_user_avatar)
                    .await;
            }
        }));
    }

    async fn load_and_handle_avatar(
        url: String,
        ctx: egui::Context,
        need_reload_avatar: Arc<AtomicBool>,
        spotify_user_avatar: Arc<Mutex<Option<TextureHandle>>>,
    ) {
        match Self::load_spotify_user_avatar(&url, &ctx).await {
            Ok(texture) => Self::handle_avatar_load_success(
                texture,
                spotify_user_avatar,
                need_reload_avatar,
                &ctx,
            ),
            Err(e) => error!("加載 Spotify 用戶頭像失敗: {:?}", e),
        }
    }

    fn handle_avatar_load_success(
        texture: TextureHandle,
        spotify_user_avatar: Arc<Mutex<Option<TextureHandle>>>,
        need_reload_avatar: Arc<AtomicBool>,
        ctx: &egui::Context,
    ) {
        info!("Spotify 用戶頭像加載成功");
        *spotify_user_avatar.lock().unwrap() = Some(texture);
        need_reload_avatar.store(false, Ordering::SeqCst);
        ctx.request_repaint();
    }

    fn check_auth_status(&mut self) {
        if !self.auth_in_progress.load(Ordering::SeqCst) {
            if let AuthStatus::Completed | AuthStatus::Failed(_) =
                self.auth_manager.get_status(&AuthPlatform::Spotify)
            {
                self.show_auth_progress = false;
                self.auth_in_progress.store(false, Ordering::SeqCst);
            }
        }
    }

    fn handle_config_errors(&mut self, ctx: &egui::Context) {
        let mut should_close_error = false;

        if let Ok(errors) = self.config_errors.try_lock() {
            if !errors.is_empty() {
                self.show_config_error_window(ctx, &errors, &mut should_close_error);
            }
        }

        if should_close_error {
            if let Ok(mut errors) = self.config_errors.try_lock() {
                errors.clear();
            }
        }
    }

    fn show_config_error_window(
        &self,
        ctx: &egui::Context,
        errors: &[String],
        should_close_error: &mut bool,
    ) {
        egui::Window::new("")
            .collapsible(false)
            .resizable(false)
            .default_size(egui::vec2(1200.0, 600.0))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.heading(egui::RichText::new("配置檢查錯誤：").size(32.0));
                    ui.add_space(20.0);

                    for error_msg in errors {
                        for error_line in error_msg.split('\n') {
                            egui::Frame::none()
                                .fill(egui::Color32::from_rgb(255, 200, 200))
                                .show(ui, |ui| {
                                    ui.add_space(10.0);
                                    ui.label(
                                        egui::RichText::new(error_line)
                                            .size(24.0)
                                            .color(egui::Color32::RED),
                                    );
                                    ui.add_space(10.0);
                                });
                            ui.add_space(5.0);
                        }
                    }

                    ui.add_space(20.0);
                    if ui
                        .add_sized(
                            [200.0, 60.0],
                            egui::Button::new(egui::RichText::new("確定").size(40.0)),
                        )
                        .clicked()
                    {
                        *should_close_error = true;
                    }
                });
            });
    }

    fn update_ui(&mut self, ctx: &egui::Context) {
        if self
            .need_repaint
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            ctx.request_repaint();
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            self.render_top_panel(ui);
        });

        self.render_side_menu(ctx);
        self.render_central_panel(ctx);
    }

    fn handle_debug_mode(&mut self) {
        if self.search_query.trim().to_lowercase() == "debug" {
            self.debug_mode = !self.debug_mode;
            set_log_level(self.debug_mode);
            self.search_query.clear();
            info!("Debug mode: {}", self.debug_mode);
        }
    }

    fn update_current_playing(&self, ctx: &egui::Context) {
        if self.should_update_current_playing()
            && self.should_detect_now_playing.load(Ordering::SeqCst)
        {
            let spotify_client = Arc::downgrade(&self.spotify_client);
            let currently_playing = Arc::downgrade(&self.currently_playing);
            let debug_mode = self.debug_mode;
            let ctx = ctx.clone();
            let spotify_authorized = Arc::downgrade(&self.spotify_authorized);
            let should_detect_now_playing = Arc::downgrade(&self.should_detect_now_playing);

            tokio::spawn(async move {
                if let (
                    Some(spotify_client),
                    Some(currently_playing),
                    Some(spotify_authorized),
                    Some(should_detect_now_playing),
                ) = (
                    spotify_client.upgrade(),
                    currently_playing.upgrade(),
                    spotify_authorized.upgrade(),
                    should_detect_now_playing.upgrade(),
                ) {
                    Self::update_and_handle_current_playing(
                        spotify_client,
                        currently_playing,
                        debug_mode,
                        ctx,
                        spotify_authorized,
                        should_detect_now_playing,
                    )
                    .await;
                }
            });
        }
    }

    async fn update_and_handle_current_playing(
        spotify_client: Arc<Mutex<Option<AuthCodeSpotify>>>,
        currently_playing: Arc<Mutex<Option<CurrentlyPlaying>>>,
        debug_mode: bool,
        ctx: egui::Context,
        spotify_authorized: Arc<AtomicBool>,
        should_detect_now_playing: Arc<AtomicBool>,
    ) {
        match update_currently_playing_wrapper(spotify_client, currently_playing, debug_mode).await
        {
            Ok(_) => {}
            Err(e) => Self::handle_current_playing_update_error(
                e,
                spotify_authorized,
                should_detect_now_playing,
            ),
        }

        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }

    fn handle_current_playing_update_error(
        e: impl std::fmt::Debug,
        spotify_authorized: Arc<AtomicBool>,
        should_detect_now_playing: Arc<AtomicBool>,
    ) {
        error!("更新當前播放失敗: {:?}", e);
        let error_str = format!("{:?}", e);
        if error_str.contains("Token 無效") || error_str.contains("需要重新授權") {
            info!("Token 無效或過期，需要重新授權");
            spotify_authorized.store(false, Ordering::SeqCst);
            should_detect_now_playing.store(false, Ordering::SeqCst);
        }
    }

    fn handle_download_status_updates(&mut self) {
        let status_updates = self.collect_status_updates();
        let completed_downloads = self.process_status_updates(&status_updates);

        for completed_beatmapset in completed_downloads {
            self.handle_completed_download(&[completed_beatmapset]);
        }

        if !status_updates.is_empty() {
            self.ctx.request_repaint();
        }
    }

    fn collect_status_updates(&mut self) -> Vec<(i32, DownloadStatus)> {
        let mut status_updates = Vec::new();
        while let Ok(update) = self.status_receiver.try_recv() {
            status_updates.push(update);
        }
        status_updates
    }

    fn process_status_updates(
        &mut self,
        status_updates: &[(i32, DownloadStatus)],
    ) -> Vec<Beatmapset> {
        let mut completed_downloads = Vec::new();
        if let Ok(guard) = self.osu_search_results.try_lock() {
            for &(beatmapset_id, status) in status_updates {
                if let Some(index) = guard.iter().position(|b| b.id == beatmapset_id) {
                    self.osu_download_statuses
                        .insert(beatmapset_id.try_into().unwrap(), status);
                    if status == DownloadStatus::Completed {
                        completed_downloads.push(guard[index].clone());
                        // 移除這兩行代碼：
                        // guard.remove(index);
                        // self.osu_download_statuses.remove(&index);
                    }
                }
            }
        }
        completed_downloads
    }

    fn handle_completed_download(&mut self, guard: &[Beatmapset]) {
        if let Some((waiting_index, waiting_beatmapset)) = self.find_waiting_download(guard) {
            self.start_waiting_download(waiting_index, waiting_beatmapset);
        }
    }

    fn find_waiting_download(&self, guard: &[Beatmapset]) -> Option<(usize, i32)> {
        self.osu_download_statuses
            .iter()
            .find(|(_, &status)| status == DownloadStatus::Waiting)
            .map(|(index, _)| (*index, guard[*index].id))
    }

    fn start_waiting_download(&mut self, waiting_index: usize, waiting_beatmapset: i32) {
        self.osu_download_statuses
            .insert(waiting_index, DownloadStatus::Downloading);
        if let Err(e) = self.download_queue_sender.try_send(waiting_beatmapset) {
            error!("無法將等待中的圖譜加入下載隊列: {:?}", e);
            self.osu_download_statuses
                .insert(waiting_index, DownloadStatus::Waiting);
        }
    }

    // 新增清理方法
    fn clean_up_resources(&mut self) {
        // 清理搜尋結果
        if let Ok(mut guard) = self.osu_search_results.try_lock() {
            guard.clear();
        }

        // 清理下載狀態
        self.osu_download_statuses.clear();

        // 清理紋理快取
        tokio::task::block_in_place(|| {
            let mut textures = futures::executor::block_on(self.cover_textures.write());
            textures.clear();
        });
    }
}

// 大致運作流程：
// 1. 初始化應用程式
//    - 載入設定
//    - 初始化 UI 元件
//    - 設置網路客戶端
//    - 初始化快取系統
// 2. 主循環
//    - 處理使用者輸入
//    - 更新認證狀態
//      - 必要時重新授權
//    - 更新使用者介面
//      - 處理頭像載入
//      - 更新播放狀態
//      - 更新搜尋結果
//    - 處理下載任務
//      - 更新下載狀態
//      - 管理下載佇列
//    - 處理錯誤和異常情況
//    - 優化性能
//      - 清理不必要的資源
//      - 管理記憶體使用
// 3. 結束程式
//    - 保存使用者設定
//    - 取消所有進行中的任務
//    - 清理所有資源
// 4. 除錯模式（可在任何時候啟用）
//    - 記錄詳細資訊
//    - 顯示額外的除錯資訊

impl SearchApp {
    fn new(
        client: Arc<tokio::sync::Mutex<Client>>,
        sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
        receiver: tokio::sync::mpsc::Receiver<(usize, Arc<TextureHandle>, (f32, f32))>,
        cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>>,
        need_repaint: Arc<AtomicBool>,
        ctx: egui::Context,
        config_errors: Arc<Mutex<Vec<String>>>,
        debug_mode: bool,
    ) -> Result<Self, AppError> {
        let texture_cache: Arc<RwLock<HashMap<String, Arc<TextureHandle>>>> =
            Arc::new(RwLock::new(HashMap::new()));
        let texture_load_queue: Arc<Mutex<BinaryHeap<Reverse<(usize, String)>>>> =
            Arc::new(Mutex::new(BinaryHeap::new()));

        let texture_cache_clone = Arc::clone(&texture_cache);
        let texture_load_queue_clone = Arc::clone(&texture_load_queue);
        let need_repaint_clone = Arc::clone(&need_repaint);
        let ctx_clone = ctx.clone();

        let spotify_icon = load_spotify_icon(&ctx);
        let config = read_config(debug_mode)?;

        let (update_check_sender, update_check_receiver) = tokio::sync::mpsc::channel(100); // 設置適當的緩衝區大小
        let mut oauth = OAuth::default();
        oauth.redirect_uri = "http://localhost:8888/callback".to_string();
        oauth.scopes = scopes!("user-read-currently-playing");

        let spotify_client = Arc::new(Mutex::new(None));
        let spotify_authorized = Arc::new(AtomicBool::new(false));
        let spotify_user_avatar = Arc::new(Mutex::new(None));
        let spotify_user_avatar_url = Arc::new(Mutex::new(None));
        let need_reload_avatar = Arc::new(AtomicBool::new(false));
        let spotify_user_name = Arc::new(Mutex::new(None));

        // 檢查並刷新 Spotify 令牌
        let client_for_refresh = client.clone();
        let spotify_client_clone = spotify_client.clone();
        let spotify_authorized_clone = spotify_authorized.clone();
        let spotify_user_avatar_url_clone = spotify_user_avatar_url.clone();
        let need_reload_avatar_clone = need_reload_avatar.clone();
        let spotify_user_name_clone = spotify_user_name.clone();
        let ctx_clone2 = ctx.clone();

        let download_directory = load_download_directory().unwrap_or_else(|| PathBuf::from("."));

        let (status_sender, status_receiver) = tokio::sync::mpsc::channel(100);
        let (download_queue_sender, download_queue_receiver) = mpsc::channel(100);

        let audio_output = OutputStream::try_default().ok();

        let scale_factor = load_scale_factor().unwrap_or(Some(2.0)).unwrap_or(2.0);

        tokio::spawn(async move {
            let client_guard = client_for_refresh.lock().await;
            match check_and_refresh_token(&client_guard, &config, "spotify").await {
                Ok(login_info) => {
                    let new_spotify = AuthCodeSpotify::new(
                        Credentials::new(&config.spotify.client_id, &config.spotify.client_secret),
                        oauth.clone(),
                    );
                    let token = Token {
                        access_token: login_info.access_token.clone(),
                        refresh_token: Some(login_info.refresh_token.clone()),
                        expires_in: TimeDelta::try_seconds(
                            (login_info.expiry_time - Utc::now()).num_seconds(),
                        )
                        .unwrap_or_default(),
                        expires_at: Some(login_info.expiry_time),
                        scopes: oauth.scopes,
                    };
                    if let Ok(mut spotify_client_guard) = spotify_client_clone.lock() {
                        *spotify_client_guard = Some(new_spotify);
                        if let Some(spotify) = spotify_client_guard.as_mut() {
                            spotify.token = Arc::new(rspotify::sync::Mutex::new(Some(token)));
                        }
                    }
                    spotify_authorized_clone.store(true, Ordering::SeqCst);

                    // 設置用戶頭像 URL 和用戶名
                    if let Some(avatar_url) = &login_info.avatar_url {
                        *spotify_user_avatar_url_clone.lock().unwrap() = Some(avatar_url.clone());
                        need_reload_avatar_clone.store(true, Ordering::SeqCst);
                    }
                    if let Some(user_name) = &login_info.user_name {
                        *spotify_user_name_clone.lock().unwrap() = Some(user_name.clone());
                    }

                    // 觸發頭像加載
                    if need_reload_avatar_clone.load(Ordering::SeqCst) {
                        if let Some(url) = spotify_user_avatar_url_clone.lock().unwrap().clone() {
                            let spotify_user_avatar_rwlock = Arc::new(RwLock::new(None));
                            let ctx_clone3 = ctx_clone2.clone();
                            let need_reload_avatar_clone2 = need_reload_avatar_clone.clone();

                            // 使用 tokio::task::spawn_blocking 來處理非 Send 的 future
                            tokio::task::spawn_blocking(move || {
                                tokio::runtime::Runtime::new().unwrap().block_on(async {
                                    if let Err(e) = SearchApp::load_spotify_avatar(
                                        &ctx_clone3,
                                        &url,
                                        spotify_user_avatar_rwlock,
                                        need_reload_avatar_clone2,
                                    )
                                    .await
                                    {
                                        error!("加載 Spotify 頭像失敗: {}", e);
                                    }
                                });
                            });
                        }
                    }
                }
                Err(e) => {
                    error!("無法刷新 Spotify 令牌: {}", e);
                    spotify_authorized_clone.store(false, Ordering::SeqCst);
                }
            }
        });

        let mut fonts = FontDefinitions::default();
        let font_data = include_bytes!("jf-openhuninn-2.0.ttf");

        fonts.font_data.insert(
            "jf-openhuninn".to_owned(),
            FontData::from_owned(font_data.to_vec()),
        );

        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "jf-openhuninn".to_owned());
        }
        if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
            family.insert(0, "jf-openhuninn".to_owned());
        }

        ctx.set_fonts(fonts);

        let mut preloaded_icons = HashMap::new();
        let icon_paths = vec![
            "spotify_icon_black.png",
            "osu!logo.png",
            "Spotify_Full_Logo_RGB_White.png",
            "Spotify_Full_Logo_RGB_Black.png",
            "osu!logo@2x.png",
            "search.png",
            "like.png",
            "liked.png",
            "expand_on.png",
            "expand_off.png",
            "play.png",
            "pause.png",
            "download.png",
            "delete.png",
            "downloading.png",
            "background1.jpg",
            "background_light2.jpg",
        ];
        for path in icon_paths {
            if let Some(texture) = Self::load_icon(&ctx, path) {
                preloaded_icons.insert(path.to_string(), texture);
            }
        }

        // 啟動異步加載任務
        tokio::spawn(async move {
            loop {
                let item = {
                    let mut queue = texture_load_queue_clone.lock().unwrap();
                    queue.pop()
                };

                if let Some(Reverse((_, url))) = item {
                    if !texture_cache_clone.read().await.contains_key(&url) {
                        match Self::load_texture_async(&ctx_clone, &url, Duration::from_secs(30))
                            .await
                        {
                            Ok(texture) => {
                                texture_cache_clone
                                    .write()
                                    .await
                                    .insert(url.clone(), Arc::new(texture));
                                need_repaint_clone.store(true, Ordering::SeqCst);
                            }
                            Err(e) => {
                                error!("載入紋理失敗: {:?}", e);
                            }
                        }
                    }
                }

                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        });

        let mut app = Self {
            // 自定義背景
            custom_background_path: None,
            custom_background: None,
            // 認證相關
            access_token: Arc::new(tokio::sync::Mutex::new(String::new())),
            auth_in_progress: Arc::new(AtomicBool::new(false)),
            auth_manager: Arc::new(AuthManager::new()),
            auth_start_time: None,
            spotify_authorized,
            spotify_client,

            // 使用者資訊
            spotify_user_avatar,
            spotify_user_avatar_url,
            spotify_user_name,

            // 搜索相關
            search_query: String::new(),
            is_searching: Arc::new(AtomicBool::new(false)),
            search_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            osu_search_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            displayed_spotify_results: 10,
            displayed_osu_results: 10,
            downloaded_maps_search: String::new(),
            playlist_search_query: String::new(),
            tracks_search_query: String::new(),
            // 播放列表和曲目
            spotify_user_playlists: Arc::new(Mutex::new(Vec::new())),
            spotify_playlist_tracks: Arc::new(Mutex::new(Vec::new())),
            spotify_liked_tracks: Arc::new(Mutex::new(Vec::new())),
            selected_playlist: None,
            currently_playing: Arc::new(Mutex::new(None)),

            // UI 狀態
            show_auth_progress: false,
            show_side_menu: false,
            side_menu_width: Some(BASE_SIDE_MENU_WIDTH),
            show_spotify_now_playing: false,
            show_playlists: false,
            show_liked_tracks: false,
            spotify_scroll_to_top: false,
            osu_scroll_to_top: false,
            global_font_size: 16.0,
            search_bar_expanded: false,
            global_volume: 0.3,
            expanded_track_index: None,
            expanded_beatmapset_index: None,
            is_beatmap_playing: false,
            scale_factor,
            is_first_update: true,
            show_downloaded_maps: false,
            expanded_map_indices: HashSet::new(),
            show_osu_search_bar: false,
            show_playlist_search_bar: false,
            show_tracks_search_bar: false,

            // 紋理和圖像
            avatar_load_handle: None,
            cover_textures,
            playlist_cover_textures: Arc::new(Mutex::new(HashMap::new())),
            default_avatar_texture: None,
            spotify_icon,
            texture_cache,
            preloaded_icons,

            // 網絡和客戶端
            client,
            listener: Arc::new(TokioMutex::new(None)),

            // 錯誤處理
            err_msg: Arc::new(tokio::sync::Mutex::new(String::new())),
            error_message: Arc::new(tokio::sync::Mutex::new(String::new())),
            config_errors,

            // 狀態管理
            initialized: false,
            need_reload_avatar,
            need_repaint,
            last_update: Arc::new(Mutex::new(None)),
            last_avatar_update: Utc::now(),
            beatmapset_download_statuses: Arc::new(Mutex::new(HashMap::new())),

            // 異步通信
            receiver: Some(receiver),
            sender,

            // UI 元素狀態
            side_menu_animation: HashMap::new(),

            // 其他功能
            debug_mode,
            ctx,
            selected_beatmapset: None,
            should_detect_now_playing: Arc::new(AtomicBool::new(false)),
            spotify_track_liked_status: Arc::new(Mutex::new(HashMap::new())),
            osu_download_statuses: HashMap::new(),
            osu_helper: OsuHelper::new(),

            // 快取
            liked_songs_cache: Arc::new(Mutex::new(None)),
            cache_ttl: Duration::from_secs(300), // 5 分鐘的緩存有效期
            texture_load_queue,

            // 更新檢查
            update_check_result: Arc::new(Mutex::new(None)),
            update_check_sender,
            update_check_receiver,
            last_background_key: String::new(),

            // 下載相關
            download_directory,
            status_sender,
            status_receiver,
            download_queue_sender,
            download_queue_receiver: Arc::new(Mutex::new(Some(download_queue_receiver))),
            download_semaphore: Arc::new(Semaphore::new(3)), // 允許3個同時下載
            current_downloads: Arc::new(AtomicUsize::new(0)),

            // 音頻播放
            audio_output,
            current_previews: Arc::new(TokioMutex::new(HashMap::new())),
            need_load_background: true,
        };
        // 檢查並加載本地頭像
        if let Some(user_name) = app.spotify_user_name.lock().unwrap().clone() {
            let avatar_path = Self::get_avatar_path(&user_name);
            if let Ok(Some(texture)) = Self::load_local_avatar(&app.ctx, &avatar_path) {
                *app.spotify_user_avatar.lock().unwrap() = Some(texture);
                app.need_reload_avatar.store(false, Ordering::SeqCst);
            }
        }

        app.load_default_avatar();
        app.start_download_processor();

        Ok(app)
    }

    fn cancel_authorization(&mut self) {
        self.auth_manager.reset(&AuthPlatform::Spotify);
        self.auth_start_time = None;
        self.auth_in_progress.store(false, Ordering::SeqCst);
        self.show_auth_progress = false;

        if let Ok(mut listener_guard) = self.listener.try_lock() {
            *listener_guard = None;
        }

        if let Ok(mut spotify_client) = self.spotify_client.try_lock() {
            *spotify_client = None;
        }

        // 確保將狀態設置為 NotStarted
        self.auth_manager
            .update_status(&AuthPlatform::Spotify, AuthStatus::NotStarted);

        error!("用戶取消了授權流程");
    }

    fn start_spotify_authorization(&mut self, ctx: egui::Context) {
        if self.auth_in_progress.load(Ordering::SeqCst) {
            info!("Spotify 授權已在進行中，請等待");
            return;
        }

        info!("開始 Spotify 授權流程");
        self.show_auth_progress = true;
        self.auth_in_progress.store(true, Ordering::SeqCst);
        self.auth_manager.reset(&AuthPlatform::Spotify);
        self.auth_start_time = Some(Instant::now());

        // 重置相關狀態
        self.spotify_authorized.store(false, Ordering::SeqCst);
        *self.spotify_user_avatar_url.lock().unwrap() = None;
        self.need_reload_avatar.store(true, Ordering::SeqCst);

        let spotify_client = self.spotify_client.clone();
        let debug_mode = self.debug_mode;
        let spotify_authorized = self.spotify_authorized.clone();
        let auth_manager = self.auth_manager.clone();
        let listener = self.listener.clone();
        let ctx_clone = ctx.clone();
        let spotify_user_avatar_url = self.spotify_user_avatar_url.clone();
        let need_reload_avatar = self.need_reload_avatar.clone();
        let spotify_user_name = self.spotify_user_name.clone();
        let auth_in_progress = self.auth_in_progress.clone();
        let spotify_user_avatar = self.spotify_user_avatar.clone();

        tokio::spawn(async move {
            // 關閉之前的監聽器（如果有的話）
            {
                let mut listener_guard = listener.lock().await;
                if let Some(l) = listener_guard.take() {
                    drop(l);
                }
            }

            let result = authorize_spotify(
                spotify_client.clone(),
                debug_mode,
                auth_manager.clone(),
                listener.clone(),
                spotify_authorized.clone(),
            )
            .await;

            match result {
                Ok((avatar_url, Some(user_name))) => {
                    info!(
                        "Spotify 授權成功，獲取到頭像 URL: {:?} 和用戶名稱: {}",
                        avatar_url, user_name
                    );
                    let avatar_path = Self::get_avatar_path(&user_name);
                    if let Some(url) = &avatar_url {
                        if let Err(e) = Self::download_and_save_avatar(url, &avatar_path).await {
                            error!("下載並保存頭像失敗: {:?}", e);
                        }
                    }
                    *spotify_user_avatar_url.lock().unwrap() = avatar_url;
                    *spotify_user_name.lock().unwrap() = Some(user_name.clone());
                    need_reload_avatar.store(true, Ordering::SeqCst);
                    spotify_authorized.store(true, Ordering::SeqCst);
                    auth_manager.update_status(&AuthPlatform::Spotify, AuthStatus::Completed);

                    // 加載本地頭像
                    if let Ok(Some(texture)) = Self::load_local_avatar(&ctx_clone, &avatar_path) {
                        let mut avatar = spotify_user_avatar.lock().unwrap();
                        *avatar = Some(texture);
                        need_reload_avatar.store(false, Ordering::SeqCst);
                    }
                }
                Ok((_, None)) => {
                    error!("Spotify 授權成功，但未獲取到用戶 ID");
                    spotify_authorized.store(true, Ordering::SeqCst);
                    auth_manager.update_status(&AuthPlatform::Spotify, AuthStatus::Completed);
                }
                Err(e) => {
                    error!("Spotify 授權失敗: {:?}", e);
                    auth_manager
                        .update_status(&AuthPlatform::Spotify, AuthStatus::Failed(e.to_string()));
                }
            }

            auth_in_progress.store(false, Ordering::SeqCst);
            ctx_clone.request_repaint();
        });
    }

    fn should_update_current_playing(&self) -> bool {
        if !self.spotify_authorized.load(Ordering::SeqCst) {
            return false; // 如果未授權，不更新
        }

        let mut last_update = self.last_update.lock().unwrap();
        if last_update.is_none() || last_update.unwrap().elapsed() > Duration::from_secs(2) {
            *last_update = Some(Instant::now());
            true
        } else {
            false
        }
    }
    //創建右鍵選單
    fn create_context_menu<F>(&self, ui: &mut egui::Ui, content: F)
    where
        F: FnOnce(&mut dyn FnMut(&str, Box<dyn FnOnce() + '_>)),
    {
        ui.style_mut()
            .text_styles
            .iter_mut()
            .for_each(|(__, font_id)| {
                font_id.size = self.global_font_size * 1.2;
            });

        ui.style_mut().spacing.item_spacing.y = 5.0; // 減少項目間的垂直間距

        ui.vertical_centered(|ui| {
            ui.add_space(5.0);

            let button_width = ui.available_width().max(100.0);
            let button_height = 30.0;

            content(&mut |label: &str, on_click: Box<dyn FnOnce() + '_>| {
                if ui
                    .add_sized(
                        [button_width, button_height],
                        egui::Button::new(
                            egui::RichText::new(label)
                                .size(self.global_font_size * 1.2)
                                .text_style(egui::TextStyle::Button),
                        ),
                    )
                    .clicked()
                {
                    on_click();
                    ui.close_menu();
                }
            });

            ui.add_space(5.0);
        });
    }

    async fn load_texture_async(
        ctx: &egui::Context,
        url: &str,
        timeout: Duration,
    ) -> Result<TextureHandle, anyhow::Error> {
        let client = reqwest::Client::new();
        let bytes = tokio::time::timeout(timeout, client.get(url).send())
            .await??
            .bytes()
            .await?;

        let image = image::load_from_memory(&bytes)?;
        let size = [image.width() as _, image.height() as _];
        let image_buffer = image.to_rgba8();
        let pixels = image_buffer.as_flat_samples();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

        let texture_options = egui::TextureOptions {
            magnification: egui::TextureFilter::Linear,
            minification: egui::TextureFilter::Linear,
            wrap_mode: TextureWrapMode::default(),
        };

        Ok(ctx.load_texture(url, color_image, texture_options))
    }

    //處理搜尋
    fn perform_search(&mut self, ctx: egui::Context) -> JoinHandle<Result<()>> {
        set_log_level(self.debug_mode); // 設置日誌級別

        let client = self.client.clone();
        let debug_mode = self.debug_mode;
        let query = self.search_query.clone();
        let search_results = self.search_results.clone();
        let osu_search_results = self.osu_search_results.clone();
        let is_searching = self.is_searching.clone();
        let need_repaint = self.need_repaint.clone();
        let err_msg = self.err_msg.clone();
        let sender = self.sender.clone();
        let spotify_client = self.spotify_client.clone(); // 添加這行
        let ctx_clone = ctx.clone(); // 在這裡克隆 ctx
        self.displayed_osu_results = 10;
        self.clear_cover_textures();
        self.expanded_beatmapset_index = None;

        info!("使用者搜尋: {}", query);

        is_searching.store(true, Ordering::SeqCst);

        tokio::spawn(async move {
            let result: Result<()> = async {
                let mut error = err_msg.lock().await;
                error.clear();
                if debug_mode {
                    debug!("除錯模式開啟");
                }

                let spotify_token = get_access_token(&*client.lock().await, debug_mode)
                    .await
                    .map_err(|e| match e {
                        SpotifyError::AccessTokenError(msg) => {
                            anyhow!("Spotify 錯誤：無法獲取 token: {}", msg)
                        }
                        SpotifyError::RequestError(e) => anyhow!("Spotify 請求錯誤：{}", e),
                        _ => anyhow!("Spotify 錯誤：{}", e),
                    })?;

                let osu_token = get_osu_token(&*client.lock().await, debug_mode)
                    .await
                    .map_err(|e| {
                        error!("獲取 Osu token 錯誤: {:?}", e);
                        anyhow!("Osu 錯誤：無法獲取 token")
                    })?;

                if let Some((beatmapset_id, _)) = parse_osu_url(&query) {
                    info!("Osu 搜尋: {}", query);

                    // 如果是 osu! URL，獲取譜面信息並進行反搜索
                    let (artist, title) = get_beatmapset_details(
                        &*client.lock().await,
                        &osu_token,
                        &beatmapset_id,
                        debug_mode,
                    )
                    .await
                    .map_err(|e| {
                        error!("獲取 Osu 譜面詳情錯誤: {:?}", e);
                        anyhow!("Osu 錯誤：獲取譜面詳情失敗")
                    })?;

                    let spotify_query = format!("{} {}", artist, title);
                    info!("Spotify 查詢 (從 osu): {}", spotify_query);

                    // 使用獲取的 artist 和 title 進行 Spotify 搜索
                    let tracks_with_cover = search_track(
                        &*client.lock().await,
                        &spotify_query,
                        &spotify_token,
                        10,
                        0,
                        debug_mode,
                    )
                    .await
                    .map(|(tracks_with_cover, _)| tracks_with_cover)
                    .map_err(|e| {
                        error!("Spotify 反搜索錯誤: {:?}", e);
                        anyhow!("Spotify 錯誤：反搜索失敗")
                    })?;

                    // 更新 Spotify 搜索結果
                    let mut search_results = search_results.lock().await;
                    *search_results = tracks_with_cover
                        .iter()
                        .map(|twc| Track {
                            name: twc.name.clone(),
                            artists: twc.artists.clone(),
                            album: Album {
                                name: twc.album_name.clone(),
                                album_type: String::new(),
                                artists: Vec::new(),
                                external_urls: HashMap::new(),
                                images: twc
                                    .cover_url
                                    .as_ref()
                                    .map(|url| {
                                        vec![Image {
                                            url: url.clone(),
                                            width: 0,
                                            height: 0,
                                        }]
                                    })
                                    .unwrap_or_default(),
                                id: String::new(),
                                release_date: String::new(),
                                total_tracks: 0,
                            },
                            external_urls: twc.external_urls.clone(),
                            index: twc.index,
                            is_liked: None, // 添加缺失的 is_liked 字段
                        })
                        .collect();

                    // 獲取 osu! beatmapset
                    let beatmapset = get_beatmapset_by_id(
                        &*client.lock().await,
                        &osu_token,
                        &beatmapset_id,
                        debug_mode,
                    )
                    .await
                    .map_err(|e| {
                        error!("獲取 Osu 譜面錯誤: {:?}", e);
                        anyhow!("Osu 錯誤：獲取譜面失敗")
                    })?;

                    let results = vec![beatmapset];
                    *osu_search_results.lock().await = results.clone();

                    let mut osu_covers = Vec::new();
                    for (index, beatmapset) in results.iter().enumerate() {
                        osu_covers.push((index, beatmapset.covers.clone()));
                    }
                    *osu_search_results.lock().await = results;

                    if let Err(e) =
                        load_osu_covers(osu_covers, ctx_clone.clone(), sender.clone()).await
                    {
                        error!("載入 osu 封面時發生錯誤: {:?}", e);
                        if debug_mode {
                            ctx_clone.request_repaint();
                            egui::Window::new("Error").show(&ctx_clone, |ui| {
                                ui.label("部分 osu 封面載入失敗:");
                                ui.label(format!("{:?}", e));
                            });
                        }
                    }
                } else {
                    // 如果不是 osu! URL，執行原有的搜索邏輯
                    let spotify_result: Result<Vec<TrackWithCover>> =
                        match is_valid_spotify_url(&query) {
                            Ok(status) => match status {
                                SpotifyUrlStatus::Valid => {
                                    info!("Spotify 查詢 (URL): {}", query);
                                    let track_id = query
                                        .split('/')
                                        .last()
                                        .unwrap_or("")
                                        .split('?')
                                        .next()
                                        .unwrap_or("");
                                    let track = get_track_info(
                                        &*client.lock().await,
                                        track_id,
                                        &spotify_token,
                                    )
                                    .await
                                    .map_err(|e| anyhow!("獲取曲目資訊錯誤: {:?}", e))?;

                                    Ok(vec![TrackWithCover {
                                        name: track.name.clone(),
                                        artists: track.artists.clone(),
                                        external_urls: track.external_urls.clone(),
                                        album_name: track.album.name.clone(),
                                        cover_url: track
                                            .album
                                            .images
                                            .first()
                                            .map(|img| img.url.clone()),
                                        index: 0, // 添加這行，給予一個固定的索引
                                    }])
                                }
                                SpotifyUrlStatus::Incomplete => {
                                    *error = "Spotify URL 不完整，請輸入完整的 URL".to_string();
                                    return Ok(());
                                }
                                SpotifyUrlStatus::Invalid => {
                                    *error = "無效的 Spotify URL".to_string();
                                    return Ok(());
                                }
                                SpotifyUrlStatus::NotSpotify => {
                                    // 執行普通搜索
                                    if !query.is_empty() {
                                        info!("Spotify 查詢 (關鍵字): {}", query);
                                        let limit = 50;
                                        let offset = 0;
                                        search_track(
                                            &*client.lock().await,
                                            &query,
                                            &spotify_token,
                                            limit,
                                            offset,
                                            debug_mode,
                                        )
                                        .await
                                        .map(|(tracks_with_cover, _)| tracks_with_cover)
                                        .map_err(|e| anyhow!("Spotify 搜索錯誤: {}", e))
                                    } else {
                                        Ok(Vec::new())
                                    }
                                }
                            },
                            Err(e) => {
                                error!("驗證 Spotify URL 時發生錯誤: {:?}", e);
                                Err(anyhow!("Spotify URL 驗證錯誤"))
                            }
                        };

                    let osu_query = match spotify_result {
                        Ok(ref tracks_with_cover) => {
                            info!("Spotify 搜索結果: {} 首曲目", tracks_with_cover.len());
                            let mut search_results = search_results.lock().await;
                            *search_results = tracks_with_cover
                                .iter()
                                .map(|twc| Track {
                                    name: twc.name.clone(),
                                    artists: twc.artists.clone(),
                                    album: Album {
                                        name: twc.album_name.clone(),
                                        album_type: String::new(),
                                        artists: Vec::new(),
                                        external_urls: HashMap::new(),
                                        images: twc
                                            .cover_url
                                            .as_ref()
                                            .map(|url| {
                                                vec![Image {
                                                    url: url.clone(),
                                                    width: 0,
                                                    height: 0,
                                                }]
                                            })
                                            .unwrap_or_default(),
                                        id: String::new(),
                                        release_date: String::new(),
                                        total_tracks: 0,
                                    },
                                    external_urls: twc.external_urls.clone(),
                                    index: twc.index,
                                    is_liked: None, // 初始化為 None
                                })
                                .collect();

                            // 檢查前十首歌曲的喜歡狀態
                            if !search_results.is_empty() {
                                let track_ids: Vec<TrackId> = search_results
                                    .iter()
                                    .take(10)
                                    .filter_map(|track| {
                                        track
                                            .external_urls
                                            .get("spotify")
                                            .and_then(|url| url.split('/').last())
                                            .and_then(|id| TrackId::from_id(id).ok())
                                    })
                                    .collect();

                                let spotify_option = {
                                    let spotify_guard = spotify_client.lock().unwrap();
                                    spotify_guard.as_ref().cloned()
                                };

                                if let Some(spotify) = spotify_option {
                                    match spotify
                                        .current_user_saved_tracks_contains(track_ids)
                                        .await
                                    {
                                        Ok(statuses) => {
                                            for (track, &is_liked) in
                                                search_results.iter_mut().zip(statuses.iter())
                                            {
                                                track.is_liked = Some(is_liked);
                                            }
                                        }
                                        Err(e) => {
                                            error!("無法檢查歌曲喜歡狀態: {:?}", e);
                                        }
                                    }
                                }
                            }

                            if matches!(is_valid_spotify_url(&query), Ok(SpotifyUrlStatus::Valid))
                                && !tracks_with_cover.is_empty()
                            {
                                let osu_query = format!(
                                    "{} {}",
                                    tracks_with_cover[0]
                                        .artists
                                        .iter()
                                        .map(|a| a.name.clone())
                                        .collect::<Vec<_>>()
                                        .join(", "),
                                    tracks_with_cover[0].name
                                );
                                info!("Osu 查詢 (從 Spotify): {}", osu_query);
                                osu_query
                            } else {
                                info!("Osu 查詢 (關鍵字): {}", query);
                                query.clone()
                            }
                        }
                        Err(e) => {
                            error!("Spotify 搜索錯誤: {:?}", e);
                            return Err(anyhow!("Spotify 錯誤：搜索失敗"));
                        }
                    };
                    let results =
                        get_beatmapsets(&*client.lock().await, &osu_token, &osu_query, debug_mode)
                            .await
                            .map_err(|e| {
                                error!("Osu 搜索錯誤: {:?}", e);
                                anyhow!("Osu 錯誤：搜索失敗")
                            })?;

                    info!("Osu 搜索結果: {} 個 beatmapsets", results.len());
                    if debug_mode {
                        debug!("Osu 搜索結果詳情: {:?}", results);
                    }

                    let mut osu_covers = Vec::new();
                    for (index, beatmapset) in results.iter().enumerate().take(10) {
                        osu_covers.push((index, beatmapset.covers.clone()));
                    }
                    *osu_search_results.lock().await = results;

                    info!("初始加載 osu 封面：共 {} 個", osu_covers.len());

                    let osu_covers_len = osu_covers.len();
                    if let Err(e) =
                        load_osu_covers(osu_covers, ctx_clone.clone(), sender.clone()).await
                    {
                        error!("載入 osu 封面時發生錯誤: {:?}", e);
                        if debug_mode {
                            ctx_clone.request_repaint();
                            egui::Window::new("Error").show(&ctx_clone, |ui| {
                                ui.label("部分 osu 封面載入失敗:");
                                ui.label(format!("{:?}", e));
                            });
                        }
                    } else {
                        info!("成功初始加載 {} 個 osu 封面", osu_covers_len);
                    }
                }

                Ok(())
            }
            .await;

            if let Err(e) = &result {
                let mut error = err_msg.lock().await;
                *error = e.to_string();
            }

            is_searching.store(false, Ordering::SeqCst);
            need_repaint.store(true, Ordering::SeqCst);
            result
        })
    }

    //顯示Spotify搜索結果
    fn display_spotify_results(&mut self, ui: &mut egui::Ui, window_size: egui::Vec2) {
        // 獲取排序後的搜索結果
        let sorted_results = self.get_sorted_spotify_results();
        let total_results = sorted_results.len();
        // 計算實際顯示的結果數量
        let displayed_results = self.displayed_spotify_results.min(total_results);

        // 顯示 Spotify 搜索結果的標題和統計信息
        self.display_spotify_header(ui, total_results, displayed_results);

        if !sorted_results.is_empty() {
            // 遍歷並顯示每個搜索結果
            for (index, track) in sorted_results.iter().take(displayed_results).enumerate() {
                self.display_spotify_track(ui, track, index);
            }
            // 顯示底部的控制元素（如"顯示更多"按鈕）
            self.display_spotify_footer(ui, displayed_results, total_results);
        } else {
            // 如果沒有搜尋結果，顯示提示信息
        };
    }

    fn get_sorted_spotify_results(&self) -> Vec<Track> {
        self.search_results
            .try_lock()
            .map(|guard| {
                let mut results = guard.clone();
                results.sort_by_key(|track| track.index);
                results
            })
            .unwrap_or_default()
    }

    fn display_spotify_header(
        &self,
        ui: &mut egui::Ui,
        total_results: usize,
        displayed_results: usize,
    ) {
        ui.horizontal(|ui| {
            // 左側：標題、結果統計和總結果數
            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.add_space(20.0);
                let text_color = if ui.visuals().dark_mode {
                    egui::Color32::from_hex("#FFFFFF").unwrap_or(egui::Color32::WHITE)
                } else {
                    egui::Color32::from_hex("#121212").unwrap_or(egui::Color32::BLACK)
                };
                ui.label(
                    egui::RichText::new(format!("總結果數: {}", total_results))
                        .size(self.global_font_size)
                        .color(text_color),
                );
                ui.label(
                    egui::RichText::new(format!("當前顯示結果數: {}", displayed_results))
                        .size(self.global_font_size)
                        .color(text_color),
                );
            });

            // 右側：Spotify logo
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let logo_key = if ui.visuals().dark_mode {
                    "Spotify_Full_Logo_RGB_White.png"
                } else {
                    "Spotify_Full_Logo_RGB_Black.png"
                };
                if let Some(spotify_logo) = self.preloaded_icons.get(logo_key) {
                    let logo_height = 70.0;
                    let aspect_ratio =
                        spotify_logo.size()[0] as f32 / spotify_logo.size()[1] as f32;
                    let logo_width = logo_height * aspect_ratio;
                    let logo_size = egui::vec2(logo_width, logo_height);

                    let image = egui::Image::new(egui::load::SizedTexture::new(
                        spotify_logo.id(),
                        logo_size,
                    ));
                    let response = ui.add(image);
                    response.on_hover_text("此區內容由Spotify提供");
                }
            });
        });
        ui.add_space(10.0);
    }

    fn display_spotify_footer(
        &mut self,
        ui: &mut egui::Ui,
        displayed_results: usize,
        total_results: usize,
    ) {
        ui.add_space(30.0);
        ui.horizontal(|ui| {
            if displayed_results < total_results {
                if ui
                    .add_sized(
                        [150.0, 40.0],
                        egui::Button::new(egui::RichText::new("顯示更多").size(18.0)),
                    )
                    .clicked()
                {
                    self.displayed_spotify_results =
                        (self.displayed_spotify_results + 10).min(total_results);
                }
            } else {
                ui.label(egui::RichText::new("已顯示所有結果").size(18.0));
            }
            ui.add_space(20.0);
            if ui
                .add_sized(
                    [150.0, 40.0],
                    egui::Button::new(egui::RichText::new("回到頂部").size(18.0)),
                )
                .clicked()
            {
                self.spotify_scroll_to_top = true;
                ui.ctx().request_repaint();
            }
        });
        ui.add_space(50.0);
    }

    fn display_spotify_track(&mut self, ui: &mut egui::Ui, track: &Track, index: usize) {
        let response = ui.add(
            egui::Button::new("")
                .frame(false)
                .min_size(egui::vec2(ui.available_width(), 100.0)),
        );

        ui.allocate_ui_at_rect(response.rect, |ui| {
            ui.horizontal(|ui| {
                self.display_album_cover(ui, track);
                ui.add_space(10.0);
                self.display_track_info(ui, track);
            });
        });

        self.draw_spotify_circular_buttons(ui, track, index, response.rect.center());

        response.context_menu(|ui| self.create_track_context_menu(ui, track));

        ui.add_space(5.0);
        ui.separator();
    }

    fn display_album_cover(&self, ui: &mut egui::Ui, track: &Track) {
        if let Some(cover_url) = track.album.images.first().map(|img| &img.url) {
            if let Ok(cache) = self.texture_cache.try_read() {
                if let Some(texture) = cache.get(cover_url) {
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(
                        texture.id(),
                        egui::Vec2::new(100.0, 100.0),
                    )));
                } else {
                    self.queue_texture_load(track.index, cover_url);
                    ui.add_sized([100.0, 100.0], egui::Spinner::new().size(32.0));
                }
            } else {
                ui.add_sized([100.0, 100.0], egui::Spinner::new().size(32.0));
            }
        }
    }

    fn queue_texture_load(&self, index: usize, cover_url: &str) {
        if let Ok(mut queue) = self.texture_load_queue.lock() {
            if !queue.iter().any(|Reverse((_, url))| url == cover_url) {
                queue.push(Reverse((index, cover_url.to_string())));
            }
        }
    }

    fn display_track_info(&mut self, ui: &mut egui::Ui, track: &Track) {
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(&track.name)
                    .font(egui::FontId::proportional(self.global_font_size * 1.0))
                    .strong(),
            );

            let artist_names = track
                .artists
                .iter()
                .map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", ");

            if ui
                .add(
                    egui::Label::new(
                        egui::RichText::new(&artist_names)
                            .font(egui::FontId::proportional(self.global_font_size * 0.9)),
                    )
                    .sense(egui::Sense::click()),
                )
                .clicked()
            {
                self.search_query = artist_names.clone();
                self.perform_search(self.ctx.clone());
            }

            ui.label(
                egui::RichText::new(&track.album.name)
                    .font(egui::FontId::proportional(self.global_font_size * 0.7)),
            );
        });
    }

    fn draw_spotify_circular_buttons(
        &mut self,
        ui: &mut egui::Ui,
        track: &Track,
        index: usize,
        center: egui::Pos2,
    ) {
        let button_size = egui::vec2(30.0, 30.0);
        let container_width = 180.0;
        let container_height = 30.0;

        let container_pos = egui::pos2(
            ui.min_rect().right() - container_width - 10.0,
            center.y - container_height / 2.0 + 30.0,
        );

        // 繪製展開按鈕
        let expand_button_rect = egui::Rect::from_min_size(
            container_pos + egui::vec2(container_width - button_size.x, 0.0),
            button_size,
        );

        if self.expanded_track_index == Some(index) {
        } else {
            // 如果當前軌道未展開，顯示展開按鈕
            if ui.put(expand_button_rect, egui::Button::new("▶")).clicked() {
                self.expanded_track_index = Some(index);
            }
        }

        if self.expanded_track_index == Some(index) {
            // 計算動畫進度
            let animation_progress = 1.0; // 暫時移除動畫，使用固定值

            // 計算動畫中的容器寬度
            let animated_width = container_width * animation_progress;
            let animated_container_rect = egui::Rect::from_min_size(
                container_pos,
                egui::vec2(animated_width, container_height),
            );

            // 如果當前軌道被展開，繪製完整的按鈕列表
            ui.painter().rect(
                animated_container_rect,
                egui::Rounding::same(10.0),
                egui::Color32::from_hex("#FFFFFF").unwrap_or(egui::Color32::WHITE),
                egui::Stroke::NONE,
            );

            let total_buttons = 4; // 減少為4個按鈕
            let spacing = animated_width / (total_buttons as f32 + 1.0);

            for i in 0..total_buttons {
                let button_center =
                    container_pos + egui::vec2((i as f32 + 1.0) * spacing, container_height / 2.0);
                let rect = egui::Rect::from_center_size(button_center, button_size);

                // 只有當按鈕完全顯示時才繪製和處理
                if rect.right() <= animated_container_rect.right() {
                    ui.painter().circle(
                        rect.center(),
                        button_size.x / 2.0,
                        egui::Color32::from_hex("#FFFFFF").unwrap_or(egui::Color32::WHITE),
                        egui::Stroke::NONE,
                    );

                    self.draw_button_icon(ui, rect, i, track);

                    let response = ui.allocate_rect(rect, egui::Sense::click());
                    if response.clicked() {
                        self.handle_button_click(i, track, index, ui.ctx().clone());
                    }
                    if response.hovered() {
                        ui.painter().circle(
                            rect.center(),
                            button_size.x / 2.0,
                            egui::Color32::from_white_alpha(200),
                            egui::Stroke::NONE,
                        );
                        let hover_text = match i {
                            0 => "開啟",
                            1 => "搜尋",
                            2 => {
                                if track.is_liked.unwrap_or(false) {
                                    "取消收藏"
                                } else {
                                    "收藏"
                                }
                            }
                            3 => "收起",
                            _ => "",
                        };
                        response.on_hover_text(hover_text);
                    }
                }
            }
        } else {
            // 如果未展開，只顯示展開按鈕
            ui.painter().rect(
                expand_button_rect,
                egui::Rounding::same(5.0),
                egui::Color32::from_hex("#FFFFFF").unwrap_or(egui::Color32::WHITE),
                egui::Stroke::NONE,
            );
            // 繪製展開圖標
            if let Some(texture) = self.preloaded_icons.get("expand_on.png") {
                let icon_size = egui::vec2(21.0, 21.0);
                let icon_rect =
                    egui::Rect::from_center_size(expand_button_rect.center(), icon_size);
                ui.painter().image(
                    texture.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::BLACK,
                );
            }
        }

        // 請求重繪以實現動畫效果
        ui.ctx().request_repaint();
    }

    fn draw_button_icon(&self, ui: &mut egui::Ui, rect: egui::Rect, index: usize, track: &Track) {
        let icon_size = egui::vec2(24.0, 24.0);
        let icon_rect = egui::Rect::from_center_size(rect.center(), icon_size);

        match index {
            0 => {
                if let Some(texture) = self.preloaded_icons.get("search.png") {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::BLACK,
                    );
                }
            }
            1 => {
                if let Some(texture) = self.preloaded_icons.get("spotify_icon_black.png") {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }
            2 => {
                let icon_key = if track.is_liked.unwrap_or(false) {
                    "liked.png"
                } else {
                    "like.png"
                };
                if let Some(texture) = self.preloaded_icons.get(icon_key) {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            }
            3 => {
                if let Some(texture) = self.preloaded_icons.get("expand_off.png") {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::BLACK,
                    );
                }
            }
            _ => {}
        }
    }

    fn handle_button_click(
        &mut self,
        index: usize,
        track: &Track,
        track_index: usize,
        ctx: egui::Context,
    ) {
        match index {
            0 => self.handle_search_click(track),
            1 => self.handle_open_click(track),
            2 => self.handle_like_click(track, track_index, ctx),
            3 => self.expanded_track_index = None, // 收起按鈕的處理邏輯
            _ => {}
        }
    }

    fn handle_search_click(&mut self, track: &Track) {
        self.search_query = track
            .external_urls
            .get("spotify")
            .cloned()
            .unwrap_or_else(|| {
                format!(
                    "{} {}",
                    track.name,
                    track
                        .artists
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            });
        self.perform_search(self.ctx.clone());
    }

    fn handle_open_click(&self, track: &Track) {
        if let Some(url) = track.external_urls.get("spotify") {
            if let Err(e) = open_spotify_url(url) {
                log::error!("無法開啟 URL: {}", e);
            }
        }
    }

    fn handle_like_click(&mut self, track: &Track, index: usize, ctx: egui::Context) {
        if self.spotify_authorized.load(Ordering::SeqCst)
            && self.spotify_client.lock().unwrap().is_some()
        {
            let track_id = track
                .external_urls
                .get("spotify")
                .and_then(|url| url.split('/').last())
                .unwrap_or("");
            let is_liked = track.is_liked.unwrap_or(false);
            self.toggle_track_like_status(track_id, is_liked, index, ctx);
        }
    }

    fn toggle_track_like_status(
        &self,
        track_id: &str,
        is_liked: bool,
        index: usize,
        ctx: egui::Context,
    ) {
        let track_id = track_id.to_string();
        let spotify_client = self.spotify_client.clone();
        let search_results = self.search_results.clone();

        tokio::spawn(async move {
            let spotify_option = {
                let spotify_guard = spotify_client.lock().unwrap();
                spotify_guard.as_ref().cloned()
            };

            if let Some(spotify) = spotify_option {
                let result = if is_liked {
                    remove_track_from_liked(&spotify, &track_id).await
                } else {
                    add_track_to_liked(&spotify, &track_id).await
                };

                match result {
                    Ok(_) => {
                        if let Ok(mut results) = search_results.try_lock() {
                            if let Some(track) = results.iter_mut().find(|t| t.index == index) {
                                track.is_liked = Some(!is_liked);
                            }
                        }
                        log::info!("成功更新曲目 {} 的收藏狀態", track_id);
                        ctx.request_repaint();
                    }
                    Err(e) => log::error!("更新曲目 {} 的收藏狀態時發生錯誤: {:?}", track_id, e),
                }
            } else {
                log::error!("無法獲取 Spotify 客戶端");
            }
        });
    }

    fn create_track_context_menu(&self, ui: &mut egui::Ui, track: &Track) {
        self.create_context_menu(ui, |add_button| {
            if let Some(url) = track.external_urls.get("spotify") {
                add_button(
                    "複製連結",
                    Box::new(move || {
                        let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
                        ctx.set_contents(url.clone()).unwrap();
                    }),
                );
                add_button(
                    "開啟",
                    Box::new(move || {
                        if let Err(e) = open_spotify_url(&url) {
                            log::error!("無法開啟 URL: {}", e);
                        }
                    }),
                );
            }
        });
    }
    //顯示osu搜索結果
    fn display_osu_results(&mut self, ui: &mut egui::Ui, window_size: egui::Vec2) {
        // 獲取排序後的搜索結果
        let sorted_results = self.get_sorted_osu_results();
        let total_results = sorted_results.len();
        // 計算實際顯示的結果數量
        let displayed_results = self.displayed_osu_results.min(total_results);

        // 顯示 osu 搜索結果的標題和統計信息
        self.display_osu_header(ui, total_results, displayed_results);

        if !sorted_results.is_empty() {
            // 檢查是否有選中的譜面集
            if let Some(selected_index) = self.selected_beatmapset {
                if let Some(selected_beatmapset) = sorted_results.get(selected_index) {
                    // 顯示選中的譜面集詳情
                    self.display_selected_beatmapset(ui, selected_beatmapset);
                } else {
                    // 如果選中的索引無效，重置選擇
                    self.selected_beatmapset = None;
                }
            } else {
                // 遍歷並顯示每個搜索結果
                for (index, beatmapset) in sorted_results.iter().take(displayed_results).enumerate()
                {
                    self.display_beatmapset(ui, beatmapset, index);
                }
                // 顯示底部的控制元素（如"顯示更多"按鈕）
                self.display_osu_footer(ui, displayed_results, total_results);
            }
        } else {
            // 如果沒搜尋結果，顯示提示信息
            ui.label("沒有搜尋結果");
        }
    }

    //顯示osu搜索結果的標題和統計信息
    fn display_osu_header(
        &self,
        ui: &mut egui::Ui,
        total_results: usize,
        displayed_results: usize,
    ) {
        ui.horizontal(|ui| {
            // 左側：結果統計和總結果數
            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.add_space(20.0);
                ui.label(
                    egui::RichText::new(format!("總結果數: {}", total_results))
                        .size(self.global_font_size)
                        .color(egui::Color32::from_hex("#FF66AA").unwrap_or(egui::Color32::WHITE)),
                );
                ui.label(
                    egui::RichText::new(format!("當前顯示結果數: {}", displayed_results))
                        .size(self.global_font_size)
                        .color(egui::Color32::from_hex("#FF66AA").unwrap_or(egui::Color32::WHITE)),
                );
            });

            // 右側：osu! logo
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(osu_logo) = self.preloaded_icons.get("osu!logo@2x.png") {
                    let logo_height = 70.0;
                    let aspect_ratio = osu_logo.size()[0] as f32 / osu_logo.size()[1] as f32;
                    let logo_width = logo_height * aspect_ratio;
                    let logo_size = egui::vec2(logo_width, logo_height);

                    let image =
                        egui::Image::new(egui::load::SizedTexture::new(osu_logo.id(), logo_size));
                    let response = ui.add(image);
                    response.on_hover_text("此區內容由osu!提供");
                }
            });
        });
        ui.add_space(10.0);
    }

    //顯示osu搜索結果的底部控制元素
    fn display_osu_footer(
        &mut self,
        ui: &mut egui::Ui,
        displayed_results: usize,
        total_results: usize,
    ) {
        ui.add_space(30.0);
        ui.horizontal(|ui| {
            if displayed_results < total_results {
                if ui
                    .add_sized(
                        [150.0, 40.0],
                        egui::Button::new(egui::RichText::new("顯示更多").size(18.0)),
                    )
                    .clicked()
                {
                    let new_displayed_results = (displayed_results + 10).min(total_results);
                    self.displayed_osu_results = new_displayed_results;
                    self.load_more_osu_covers(displayed_results, new_displayed_results);
                }
            } else {
                ui.label(egui::RichText::new("已顯示所有結果").size(18.0));
            }

            ui.add_space(20.0);

            if ui
                .add_sized(
                    [150.0, 40.0],
                    egui::Button::new(egui::RichText::new("回到頂部").size(18.0)),
                )
                .clicked()
            {
                self.osu_scroll_to_top = true;
            }
        });
        ui.add_space(50.0);
    }

    //獲取排序後的osu搜索結果
    fn get_sorted_osu_results(&self) -> Vec<Beatmapset> {
        if let Ok(osu_search_results_guard) = self.osu_search_results.try_lock() {
            let results = osu_search_results_guard.clone();
            results
        } else {
            error!("無法獲取 osu 搜索結果鎖");
            Vec::new()
        }
    }

    //加載更多osu封面
    fn load_more_osu_covers(&self, start: usize, end: usize) {
        if let Ok(osu_search_results_guard) = self.osu_search_results.try_lock() {
            let mut osu_covers = Vec::new();
            for (index, beatmapset) in osu_search_results_guard
                .iter()
                .enumerate()
                .skip(start)
                .take(end - start)
            {
                osu_covers.push((index, beatmapset.covers.clone()));
            }

            // 新增：記錄本次加載的封面數量
            let loaded_covers_count = osu_covers.len();
            info!(
                "正在加載更多 osu 封面：從 {} 到 {}，共 {} 個",
                start, end, loaded_covers_count
            );

            let sender_clone = self.sender.clone();
            let debug_mode = self.debug_mode;
            let need_repaint = self.need_repaint.clone();
            let ctx = self.ctx.clone();

            tokio::spawn(async move {
                if let Err(e) = load_osu_covers(osu_covers, ctx.clone(), sender_clone).await {
                    error!("載入更多 osu 封面時發生錯誤: {:?}", e);
                    if debug_mode {
                        error!("載入更多 osu 封面錯誤: {:?}", e);
                    }
                } else {
                    // 新增：記錄成功加載的封面數量
                    info!("成功加載 {} 個 osu 封面", loaded_covers_count);
                }
                need_repaint.store(true, std::sync::atomic::Ordering::SeqCst);
            });
        }
    }

    //顯示osu譜面集
    fn display_beatmapset(&mut self, ui: &mut egui::Ui, beatmapset: &Beatmapset, index: usize) {
        let response = ui.add(
            egui::Button::new("")
                .frame(false)
                .min_size(egui::vec2(ui.available_width(), 100.0)),
        );

        if response.clicked() {
            self.selected_beatmapset = Some(index);
        }

        ui.allocate_ui_at_rect(response.rect, |ui| {
            ui.horizontal(|ui| {
                if !self.show_side_menu {
                    ui.vertical(|ui| {
                        let is_image_loaded = if let Ok(textures) = self.cover_textures.try_read() {
                            textures.get(&index).map_or(false, |opt| opt.is_some())
                        } else {
                            false
                        };

                        if is_image_loaded {
                            if let Ok(textures) = self.cover_textures.try_read() {
                                if let Some(Some((texture, size))) = textures.get(&index) {
                                    let max_height = 100.0;
                                    let aspect_ratio = size.0 / size.1;
                                    let image_size =
                                        egui::Vec2::new(max_height * aspect_ratio, max_height);
                                    let image_response = ui.add(
                                        egui::Image::new((texture.id(), image_size))
                                            .sense(egui::Sense::click()),
                                    );
                                    if image_response.clicked() {
                                        self.selected_beatmapset = Some(index);
                                    }
                                }
                            }
                        } else {
                            ui.add_sized([100.0, 100.0], egui::Spinner::new().size(32.0));
                        }
                    });

                    ui.add_space(10.0);
                }

                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(&beatmapset.title)
                            .font(egui::FontId::proportional(self.global_font_size * 1.0))
                            .strong(),
                    );
                    if ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(&beatmapset.artist)
                                    .font(egui::FontId::proportional(self.global_font_size * 0.9)),
                            )
                            .sense(egui::Sense::click()),
                        )
                        .clicked()
                    {
                        self.search_query = beatmapset.artist.clone();
                        self.perform_search(self.ctx.clone());
                    }
                    ui.label(
                        egui::RichText::new(format!("by {}", beatmapset.creator))
                            .font(egui::FontId::proportional(self.global_font_size * 0.7)),
                    );
                });
            });
        });
        self.draw_osu_circular_buttons(ui, beatmapset, index, response.rect.center());

        ui.add_space(5.0);
        ui.separator();
    }

    //顯示osu譜面集按鈕
    fn draw_osu_circular_buttons(
        &mut self,
        ui: &mut egui::Ui,
        beatmapset: &Beatmapset,
        index: usize,
        center: egui::Pos2,
    ) {
        let button_size = egui::vec2(30.0, 30.0);
        let container_width = 180.0;
        let container_height = 30.0;

        let container_pos = egui::pos2(
            ui.min_rect().right() - container_width - 10.0,
            center.y - container_height / 2.0 + 30.0,
        );

        // 繪製展開按鈕
        let expand_button_rect = egui::Rect::from_min_size(
            container_pos + egui::vec2(container_width - button_size.x, 0.0),
            button_size,
        );

        if self.expanded_beatmapset_index == Some(index) {
        } else {
            // 如果當前譜面集未展開，顯示展開按鈕
            if ui.put(expand_button_rect, egui::Button::new("▶")).clicked() {
                self.expanded_beatmapset_index = Some(index);
            }
        }

        if self.expanded_beatmapset_index == Some(index) {
            // 計算動畫進度
            let animation_progress = 1.0; // 暫時移除動畫，使用固定值

            // 計算動畫中的容器寬度
            let animated_width = container_width * animation_progress;
            let animated_container_rect = egui::Rect::from_min_size(
                container_pos,
                egui::vec2(animated_width, container_height),
            );

            // 如果當前譜面集被展開，繪製完整的按鈕列表
            ui.painter().rect(
                animated_container_rect,
                egui::Rounding::same(10.0),
                egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                egui::Stroke::NONE,
            );

            let total_buttons = 5; // 增加到5個按鈕
            let spacing = animated_width / (total_buttons as f32 + 1.0);

            for i in 0..total_buttons {
                let button_center =
                    container_pos + egui::vec2((i as f32 + 1.0) * spacing, container_height / 2.0);
                let rect = egui::Rect::from_center_size(button_center, button_size);

                // 只有當按鈕完全顯示時才繪製和處理
                if rect.right() <= animated_container_rect.right() {
                    ui.painter().circle(
                        rect.center(),
                        button_size.x / 2.0,
                        egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                        egui::Stroke::NONE,
                    );

                    self.draw_osu_button_icon(ui, rect, i, beatmapset);

                    let response = ui.allocate_rect(rect, egui::Sense::click());
                    if response.clicked() {
                        self.handle_osu_button_click(i, beatmapset, ui.ctx().clone());
                    }
                    if response.hovered() {
                        ui.painter().circle(
                            rect.center(),
                            button_size.x / 2.0,
                            egui::Color32::from_rgb(255, 204, 221), // 淺粉色
                            egui::Stroke::NONE,
                        );
                        let hover_text = match i {
                            0 => "播放預覽",
                            1 => "在osu!中打開",
                            2 => {
                                if self.is_beatmap_downloaded(beatmapset.id) {
                                    "刪除"
                                } else {
                                    "下載"
                                }
                            }
                            3 => "以此尋找",
                            4 => "收起",
                            _ => "",
                        };
                        response.on_hover_text(hover_text);
                    }
                }
            }
        } else {
            // 如果未展開，只顯示展開按鈕
            ui.painter().rect(
                expand_button_rect,
                egui::Rounding::same(5.0),
                egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                egui::Stroke::NONE,
            );
            // 繪製展開圖標
            if let Some(texture) = self.preloaded_icons.get("expand_on.png") {
                let icon_size = egui::vec2(21.0, 21.0);
                let icon_rect =
                    egui::Rect::from_center_size(expand_button_rect.center(), icon_size);
                ui.painter().image(
                    texture.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                );
            }
        }

        // 請求重繪以實現動畫效果
        ui.ctx().request_repaint();
    }

    fn draw_osu_button_icon(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        index: usize,
        beatmapset: &Beatmapset,
    ) {
        let icon_size = egui::vec2(24.0, 24.0);
        let icon_rect = egui::Rect::from_center_size(rect.center(), icon_size);

        match index {
            0 => {
                let icon_key = if self.is_beatmap_playing {
                    "pause.png"
                } else {
                    "play.png"
                };
                if let Some(texture) = self.preloaded_icons.get(icon_key) {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                    );
                }
            }
            1 => {
                if let Some(texture) = self.preloaded_icons.get("osu!logo@2x.png") {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                    );
                }
            }
            2 => {
                let icon_key = if self.is_beatmap_downloaded(beatmapset.id) {
                    "delete.png"
                } else if self.get_download_status(beatmapset.id) == DownloadStatus::Downloading {
                    "downloading.png"
                } else {
                    "download.png"
                };
                if let Some(texture) = self.preloaded_icons.get(icon_key) {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                    );
                }
            }
            3 => {
                if let Some(texture) = self.preloaded_icons.get("search.png") {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                    );
                }
            }
            4 => {
                if let Some(texture) = self.preloaded_icons.get("expand_off.png") {
                    ui.painter().image(
                        texture.id(),
                        icon_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::from_hex("#FF66AA").unwrap(), // 使用HEX #FF66AA
                    );
                }
            }
            _ => {}
        }
    }

    fn handle_osu_button_click(
        &mut self,
        index: usize,
        beatmapset: &Beatmapset,
        ctx: egui::Context,
    ) {
        match index {
            0 => self.handle_osu_preview_click(beatmapset),
            1 => self.handle_osu_open_click(beatmapset),
            2 => self.handle_osu_download_click(beatmapset, ctx),
            3 => self.handle_osu_search_click(beatmapset),
            4 => self.expanded_beatmapset_index = None, // 收起按鈕的處理邏輯
            _ => {}
        }
    }

    fn handle_osu_search_click(&mut self, beatmapset: &Beatmapset) {
        self.expanded_beatmapset_index = None;
        self.search_query = if beatmapset.id != 0 {
            format!("https://osu.ppy.sh/beatmapsets/{}", beatmapset.id)
        } else {
            format!("{} {}", beatmapset.artist, beatmapset.title)
        };
        self.perform_search(self.ctx.clone());
    }

    fn handle_osu_preview_click(&mut self, beatmapset: &Beatmapset) {
        // 實現預覽播放邏輯
        if let Some(stream_handle) = self.audio_output.as_ref().map(|(_, handle)| handle.clone()) {
            let beatmapset_id = beatmapset.id;
            let volume = self.global_volume;
            let current_previews = self.current_previews.clone();
            let is_playing = self.is_beatmap_playing;

            tokio::spawn(async move {
                if is_playing {
                    // 如果正在播放，則停止
                    if let Some(sink) = current_previews.lock().await.get_mut(&beatmapset_id) {
                        sink.stop();
                    }
                } else {
                    // 如果沒有播放，則開始播放
                    match preview_beatmap(beatmapset_id, &stream_handle, volume).await {
                        Ok(sink) => {
                            let mut previews = current_previews.lock().await;
                            if let Some(old_sink) = previews.insert(beatmapset_id, sink) {
                                old_sink.stop();
                            }
                            if let Some(new_sink) = previews.get_mut(&beatmapset_id) {
                                new_sink.play();
                            }
                        }
                        Err(e) => error!("預覽播放失敗: {:?}", e),
                    }
                }
            });
        }
        // 切換播放狀態
        self.is_beatmap_playing = !self.is_beatmap_playing;
    }

    fn handle_osu_open_click(&self, beatmapset: &Beatmapset) {
        let url = format!("https://osu.ppy.sh/beatmapsets/{}", beatmapset.id);
        if let Err(e) = open::that(url) {
            error!("無法在osu!中打開譜面: {:?}", e);
        }
    }

    fn handle_osu_download_click(&mut self, beatmapset: &Beatmapset, ctx: egui::Context) {
        let beatmapset_id = beatmapset.id;
        if self.is_beatmap_downloaded(beatmapset_id) {
            // 如果已下載,則刪除
            match delete_beatmap(&self.download_directory, beatmapset_id) {
                Ok(_) => {
                    info!("成功刪除譜面 {}", beatmapset_id);
                    self.beatmapset_download_statuses
                        .lock()
                        .unwrap()
                        .insert(beatmapset_id, DownloadStatus::NotStarted);
                }
                Err(e) => {
                    error!("無法刪除譜面 {}: {:?}", beatmapset_id, e);
                }
            }
        } else {
            // 如果未下載,則開始下載
            info!("將譜面 {} 加入下載隊列", beatmapset_id);
            let current_downloads = self.current_downloads.load(Ordering::SeqCst);
            if current_downloads < 3 {
                self.beatmapset_download_statuses
                    .lock()
                    .unwrap()
                    .insert(beatmapset_id, DownloadStatus::Downloading);
            } else {
                self.beatmapset_download_statuses
                    .lock()
                    .unwrap()
                    .insert(beatmapset_id, DownloadStatus::Waiting);
            }
            if let Err(e) = self.download_queue_sender.try_send(beatmapset_id) {
                error!("無法將譜面加入下載隊列: {:?}", e);
                self.beatmapset_download_statuses
                    .lock()
                    .unwrap()
                    .insert(beatmapset_id, DownloadStatus::NotStarted);
            }
        }
        ctx.request_repaint();
    }

    fn is_beatmap_downloaded(&self, beatmapset_id: i32) -> bool {
        osu::is_beatmap_downloaded(&self.download_directory, beatmapset_id)
    }

    fn get_download_status(&self, beatmapset_id: i32) -> DownloadStatus {
        if osu::is_beatmap_downloaded(&self.download_directory, beatmapset_id) {
            DownloadStatus::Completed
        } else {
            self.beatmapset_download_statuses
                .lock()
                .unwrap()
                .get(&beatmapset_id)
                .cloned()
                .unwrap_or(DownloadStatus::NotStarted)
        }
    }

    fn start_download_processor(&self) {
        let download_queue_receiver = self.download_queue_receiver.clone();
        let download_directory = self.download_directory.clone();
        let status_sender = self.status_sender.clone();
        let semaphore = self.download_semaphore.clone();
        let current_downloads = self.current_downloads.clone();
        let beatmapset_download_statuses = self.beatmapset_download_statuses.clone();
        let osu_search_results = self.osu_search_results.clone();

        tokio::spawn(async move {
            let mut receiver = match download_queue_receiver.lock().unwrap().take() {
                Some(r) => r,
                None => {
                    error!("下載隊列接收器已被關閉");
                    return;
                }
            };

            while let Some(beatmapset_id) = receiver.recv().await {
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(e) => {
                        error!("無法獲取下載許可: {:?}", e);
                        continue;
                    }
                };

                let download_directory = download_directory.clone();
                let status_sender = status_sender.clone();
                let current_downloads = current_downloads.clone();
                let beatmapset_download_statuses = beatmapset_download_statuses.clone();
                let osu_search_results = osu_search_results.clone();

                current_downloads.fetch_add(1, Ordering::SeqCst);
                if let Err(e) = status_sender
                    .send((beatmapset_id, DownloadStatus::Downloading))
                    .await
                {
                    error!("無法發送下載狀態: {:?}", e);
                }

                tokio::spawn(async move {
                    let status_sender_clone = status_sender.clone();
                    let download_result = tokio::time::timeout(
                        std::time::Duration::from_secs(300),
                        osu::download_beatmap(beatmapset_id, &download_directory, {
                            let status_sender = status_sender.clone();
                            move |status| {
                                let beatmapset_id = beatmapset_id;
                                let status_sender = status_sender.clone();
                                tokio::spawn(async move {
                                    if let Err(e) =
                                        status_sender.send((beatmapset_id, status)).await
                                    {
                                        error!("無法發送下載狀態更新: {:?}", e);
                                    }
                                });
                            }
                        }),
                    )
                    .await;

                    match download_result {
                        Ok(Ok(_)) => {
                            info!("圖譜 {} 下載成功", beatmapset_id);

                            {
                                let search_results = osu_search_results.lock().await;
                                let results_count_before = search_results.len();

                                beatmapset_download_statuses
                                    .lock()
                                    .unwrap()
                                    .insert(beatmapset_id, DownloadStatus::Completed);

                                let results_count_after = search_results.len();

                                if results_count_before != results_count_after {
                                    error!(
                                        "警告：下載完成後搜索結果數量發生變化。之前：{}，之後：{}",
                                        results_count_before, results_count_after
                                    );
                                } else {
                                    info!("搜索結果數量未變化，保持為 {}", results_count_after);
                                }
                            }

                            if let Err(e) = status_sender_clone
                                .send((beatmapset_id, DownloadStatus::Completed))
                                .await
                            {
                                error!("無法發送下載完成狀態: {:?}", e);
                            }
                        }
                        Ok(Err(e)) => {
                            error!("圖譜 {} 下載失敗: {:?}", beatmapset_id, e);
                            beatmapset_download_statuses
                                .lock()
                                .unwrap()
                                .insert(beatmapset_id, DownloadStatus::NotStarted);
                            if let Err(e) = status_sender_clone
                                .send((beatmapset_id, DownloadStatus::NotStarted))
                                .await
                            {
                                error!("無法發送下載失敗狀態: {:?}", e);
                            }
                        }
                        Err(_) => {
                            error!("圖譜 {} 下載超時", beatmapset_id);
                            beatmapset_download_statuses
                                .lock()
                                .unwrap()
                                .insert(beatmapset_id, DownloadStatus::NotStarted);
                            if let Err(e) = status_sender_clone
                                .send((beatmapset_id, DownloadStatus::NotStarted))
                                .await
                            {
                                error!("無法發送下載超時狀態: {:?}", e);
                            }
                        }
                    }

                    current_downloads.fetch_sub(1, Ordering::SeqCst);
                    drop(permit);
                });
            }
        });
    }

    //顯示osu譜面集詳情
    fn display_selected_beatmapset(&mut self, ui: &mut egui::Ui, beatmapset: &Beatmapset) {
        let beatmap_info = print_beatmap_info_gui(beatmapset);

        ui.heading(
            egui::RichText::new(format!("{} - {}", beatmap_info.title, beatmap_info.artist))
                .font(egui::FontId::proportional(self.global_font_size * 1.1)),
        );
        ui.label(
            egui::RichText::new(format!("by {}", beatmap_info.creator))
                .font(egui::FontId::proportional(self.global_font_size * 0.9)),
        );
        ui.add_space(10.0);

        for beatmap_info in beatmap_info.beatmaps {
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(beatmap_info)
                    .font(egui::FontId::proportional(self.global_font_size * 1.0)),
            );
            ui.add_space(10.0);
            ui.separator();
        }
        if ui
            .add_sized(
                [100.0, 40.0],
                egui::Button::new(
                    egui::RichText::new("Back")
                        .font(egui::FontId::proportional(self.global_font_size * 1.0)),
                ),
            )
            .clicked()
        {
            self.selected_beatmapset = None;
        }
    }

    //清除封面紋理
    fn clear_cover_textures(&self) {
        if let Ok(mut textures) = self.cover_textures.try_write() {
            textures.clear();
        }
    }

    //加載默認頭像
    fn load_default_avatar(&mut self) {
        let default_avatar_bytes = include_bytes!("assets/login.png");
        let default_avatar_image = image::load_from_memory(default_avatar_bytes).unwrap();
        let default_avatar_size = [
            default_avatar_image.width() as _,
            default_avatar_image.height() as _,
        ];
        let default_avatar_pixels = default_avatar_image.to_rgba8();
        self.default_avatar_texture = Some(self.ctx.load_texture(
            "default_avatar",
            egui::ColorImage::from_rgba_unmultiplied(
                default_avatar_size,
                default_avatar_pixels.as_flat_samples().as_slice(),
            ),
            egui::TextureOptions::default(),
        ));
    }
    //渲染頂部面板
    fn render_top_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                // 渲染側邊選單按鈕
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(BUTTON_SIZE, BUTTON_SIZE),
                    egui::Sense::click(),
                );
                if ui.is_rect_visible(rect) {
                    let visuals = ui.style().interact(&response);
                    let animation_progress = self.side_menu_animation.entry(ui.id()).or_insert(0.0);
                    *animation_progress = if response.hovered() {
                        (*animation_progress + ui.input(|i| i.unstable_dt) * ANIMATION_SPEED)
                            .min(1.0)
                    } else {
                        (*animation_progress - ui.input(|i| i.unstable_dt) * ANIMATION_SPEED)
                            .max(0.0)
                    };
                    let color = egui::Color32::from_rgba_unmultiplied(
                        255,
                        255,
                        255,
                        (255.0 * *animation_progress) as u8,
                    );
                    ui.painter().rect_filled(
                        rect.expand(*animation_progress * 4.0),
                        visuals.rounding,
                        color,
                    );
                    let font_id = egui::FontId::proportional(24.0);
                    let galley =
                        ui.painter()
                            .layout_no_wrap("☰".to_string(), font_id, visuals.text_color());
                    let text_pos = rect.center() - galley.size() / 2.0;
                    ui.painter().galley(text_pos, galley, visuals.text_color());
                }
                if response.clicked() {
                    self.show_side_menu = !self.show_side_menu;
                    info!(
                        "Side menu button clicked. New state: {}",
                        self.show_side_menu
                    );
                }

                ui.add_space(10.0);

                // 渲染搜索按鈕和搜索欄
                let search_button = ui.add(egui::Button::new("🔍").frame(false));
                if search_button.clicked() {
                    self.search_bar_expanded = !self.search_bar_expanded;
                }
                if self.search_bar_expanded {
                    let available_width = ui.available_width() * SEARCH_BAR_WIDTH_RATIO;
                    ui.add_space(10.0);
                    ui.allocate_ui(egui::Vec2::new(available_width, 32.0), |ui| {
                        let ctx = ui.ctx().clone();
                        self.render_search_bar(ui, &ctx);
                    });
                }

                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center).with_main_justify(true),
                    |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if self.spotify_authorized.load(Ordering::SeqCst) {
                                self.render_logged_in_user(ui);

                                // 渲染正在播放按鈕
                                let now_playing_button = ui.add(
                                    egui::Button::new(egui::RichText::new("🎵").size(16.0))
                                        .min_size(egui::vec2(32.0, 32.0))
                                        .frame(false),
                                );
                                if now_playing_button.clicked() {
                                    ui.memory_mut(|mem| {
                                        mem.toggle_popup(egui::Id::new("now_playing_popup"))
                                    });
                                    self.should_detect_now_playing.store(true, Ordering::SeqCst);
                                }
                                if now_playing_button.hovered() {
                                    ui.painter().rect_stroke(
                                        now_playing_button.rect,
                                        egui::Rounding::same(4.0),
                                        egui::Stroke::new(1.0, egui::Color32::LIGHT_BLUE),
                                    );
                                }
                                self.render_now_playing_popup(ui, &now_playing_button);
                            } else {
                                self.render_guest_user(ui);
                            }
                        });
                    },
                );
            });
        });
    }

    fn render_side_menu(&mut self, ctx: &egui::Context) {
        let current_width = self.side_menu_width.unwrap_or(BASE_SIDE_MENU_WIDTH);

        egui::SidePanel::left("side_menu")
            .resizable(true)
            .min_width(MIN_SIDE_MENU_WIDTH)
            .max_width(MAX_SIDE_MENU_WIDTH)
            .default_width(current_width)
            .show_animated(ctx, self.show_side_menu, |ui| {
                let new_width = ui.available_width();

                // 只有當用戶手動調整寬度時才更新
                if (new_width - current_width).abs() > 1.0 && ui.input(|i| i.pointer.any_down()) {
                    self.side_menu_width = Some(new_width);
                    info!("側邊欄寬度已更新為: {:.2}", new_width);
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.set_min_width(current_width - 20.0);
                        self.render_side_menu_content(ui);
                    });
            });
    }

    fn render_side_menu_content(&mut self, ui: &mut egui::Ui) {
        if self.show_downloaded_maps {
            self.render_downloaded_maps_list(ui);
        } else if self.show_liked_tracks || self.selected_playlist.is_some() {
            self.render_playlist_content(ui);
        } else if self.show_playlists {
            self.render_playlists(ui);
        } else {
            self.render_main_menu(ui);
        }
    }

    fn render_main_menu(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let button_size = egui::vec2(40.0, 40.0);
                let (rect, response) = ui.allocate_exact_size(button_size, egui::Sense::click());

                if ui.is_rect_visible(rect) {
                    let visuals = ui.style().interact(&response);
                    let animation_progress = self.side_menu_animation.entry(ui.id()).or_insert(0.0);

                    if response.hovered() {
                        *animation_progress =
                            (*animation_progress + ui.input(|i| i.unstable_dt) * 4.0).min(1.0);
                    } else {
                        *animation_progress =
                            (*animation_progress - ui.input(|i| i.unstable_dt) * 4.0).max(0.0);
                    }

                    let color = egui::Color32::from_rgba_unmultiplied(
                        255,
                        255,
                        255,
                        (255.0 * *animation_progress) as u8,
                    );

                    ui.painter().rect_filled(
                        rect.expand(*animation_progress * 4.0),
                        visuals.rounding,
                        color,
                    );

                    let font_id = egui::FontId::proportional(24.0);
                    let galley =
                        ui.painter()
                            .layout_no_wrap("☰".to_string(), font_id, visuals.text_color());

                    let text_pos = rect.center() - galley.size() / 2.0;
                    ui.painter().galley(text_pos, galley, visuals.text_color());
                }

                if response.clicked() {
                    self.show_side_menu = false;
                    info!("側邊選單關閉按鈕被點擊。新狀態: false");
                }
            });
        });

        ui.style_mut().spacing.item_spacing.y = 8.0;

        // Spotify 折疊式視窗
        egui::CollapsingHeader::new(egui::RichText::new("🎵 Spotify").size(20.0))
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(5.0);
                if self
                    .create_auth_button(ui, "Search", "spotify_icon_black.png")
                    .clicked()
                {
                    info!("點擊了: Spotify 搜尋");
                    self.show_side_menu = false;
                    self.osu_helper.show = false;
                }
                if self
                    .create_auth_button(ui, "Playlists", "spotify_icon_black.png")
                    .clicked()
                {
                    info!("點擊了: Spotify 播放清單");
                    self.show_playlists = true;
                    self.load_user_playlists();
                    self.osu_helper.show = false;
                }
            });

        // Osu 折疊式視窗
        egui::CollapsingHeader::new(egui::RichText::new("🎮 Osu").size(20.0))
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(5.0);
                if self
                    .create_auth_button(ui, "Osu Helper", "osu!logo.png")
                    .clicked()
                {
                    info!("點擊了: Osu Helper");
                    self.osu_helper.show = true;
                    self.show_side_menu = false;
                }

                ui.add_space(5.0);
                if self
                    .create_auth_button(ui, "已下載圖譜", "osu!logo.png")
                    .clicked()
                {
                    info!("點擊了: 已下載圖譜");
                    self.show_downloaded_maps = true;
                }
            });

        // Settings 折疊式視窗
        egui::CollapsingHeader::new(egui::RichText::new("Settings").size(20.0))
            .default_open(true)
            .show(ui, |ui| {
                ui.add_space(5.0);

                // 整體縮放設置
                ui.horizontal(|ui| {
                    ui.label("整體縮放:");
                    if ui.button("-").clicked() {
                        self.scale_factor = (self.scale_factor - 0.1).max(0.5);
                        ui.ctx().set_pixels_per_point(self.scale_factor);
                        if let Err(e) = save_scale_factor(self.scale_factor) {
                            error!("保存縮放因子失敗: {:?}", e);
                        }
                    }
                    ui.label(format!("{:.2}", self.scale_factor));
                    if ui.button("+").clicked() {
                        self.scale_factor = (self.scale_factor + 0.1).min(3.0);
                        ui.ctx().set_pixels_per_point(self.scale_factor);
                        if let Err(e) = save_scale_factor(self.scale_factor) {
                            error!("保存縮放因子失敗: {:?}", e);
                        }
                    }
                });

                ui.add_space(10.0);

                // 音量控制
                ui.horizontal(|ui| {
                    ui.label("音量:");
                    if ui
                        .add(egui::Slider::new(&mut self.global_volume, 0.01..=1.0))
                        .changed()
                    {
                        self.update_all_sinks_volume();
                    }
                });

                ui.add_space(10.0);

                // Debug 模式設置
                let mut debug_mode = self.debug_mode;
                ui.checkbox(&mut debug_mode, "Debug Mode");
                if debug_mode != self.debug_mode {
                    self.debug_mode = debug_mode;
                    set_log_level(self.debug_mode);
                    info!("Debug mode: {}", self.debug_mode);
                }

                ui.add_space(10.0);

                // 下載目錄設置
                ui.horizontal(|ui| {
                    ui.label("圖譜下載目錄:");
                    if ui.button("更改").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_folder() {
                            self.download_directory = path;
                            if let Err(e) = save_download_directory(&self.download_directory) {
                                error!("保存下載目錄失敗: {:?}", e);
                            }
                            info!("下載目錄已更改為: {:?}", self.download_directory);
                        }
                    }
                });
                ui.add_space(5.0);
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    let path_str = self.download_directory.to_string_lossy().to_string();
                    let available_width = ui.available_width();

                    let mut lines = Vec::new();
                    let mut current_line = String::new();
                    for word in path_str.split(std::path::MAIN_SEPARATOR) {
                        let test_line = if current_line.is_empty() {
                            word.to_string()
                        } else {
                            format!("{}{}{}", current_line, std::path::MAIN_SEPARATOR, word)
                        };

                        let galley = ui.painter().layout_no_wrap(
                            test_line.clone(),
                            egui::FontId::default(),
                            ui.style().visuals.text_color(),
                        );
                        if galley.rect.width() <= available_width {
                            current_line = test_line;
                        } else {
                            if !current_line.is_empty() {
                                lines.push(current_line);
                            }
                            current_line = word.to_string();
                        }
                    }
                    if !current_line.is_empty() {
                        lines.push(current_line);
                    }

                    for line in lines {
                        ui.label(line);
                    }
                });

                ui.add_space(10.0);

                // 自定義背景設置
                ui.horizontal(|ui| {
                    ui.label("背景圖片:");
                    if ui.button("選擇背景").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("圖片", &["png", "jpg", "jpeg"])
                            .pick_file()
                        {
                            self.custom_background_path = Some(path.clone());
                            if let Err(e) = self.load_custom_background(ui.ctx()) {
                                error!("加載背景失敗: {:?}", e);
                                self.custom_background_path = None;

                                // 顯示錯誤視窗
                                let error_window = egui::Window::new("錯誤")
                                    .collapsible(false)
                                    .resizable(false);
                                error_window.show(ui.ctx(), |ui| {
                                    ui.label("無法讀取自定義背景,已恢復使用預設背景。");
                                    if ui.button("確認").clicked() {
                                        ui.close_menu();
                                    }
                                });
                            } else {
                                info!("自定義背景已設置: {:?}", path);
                                if let Err(e) = save_background_path(&self.custom_background_path) {
                                    error!("保存背景位置失敗: {:?}", e);
                                }
                            }
                        }
                    }
                    if ui.button("恢復預設背景").clicked() {
                        self.custom_background_path = None;
                        self.custom_background = None;
                        if let Err(e) = save_background_path(&None) {
                            error!("保存背景位置失敗: {:?}", e);
                        }
                        info!("已恢復使用預設背景");
                    }
                });
                if let Some(path) = &self.custom_background_path {
                    ui.label(format!("當前背景: {}", path.to_string_lossy()));
                } else {
                    ui.label("當前使用預設背景");
                }

                if ui.button("About").clicked() {
                    info!("點擊了: 關於");
                    self.show_side_menu = false;
                    self.osu_helper.show = false;
                }
            });
    }

    fn render_downloaded_maps_list(&mut self, ui: &mut egui::Ui) {
        let fixed_width = BASE_SIDE_MENU_WIDTH;

        ui.vertical(|ui| {
            ui.set_width(fixed_width);

            // 頂部標題列
            ui.horizontal(|ui| {
                if ui.button("< 返回").clicked() {
                    self.show_downloaded_maps = false;
                    self.show_side_menu = true;
                }
                ui.heading("已下載的圖譜");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(search_icon) = self.preloaded_icons.get("search.png") {
                        if ui
                            .add(egui::ImageButton::new(egui::load::SizedTexture::new(
                                search_icon.id(),
                                egui::vec2(16.0, 16.0),
                            )))
                            .clicked()
                        {
                            self.show_osu_search_bar = !self.show_osu_search_bar;
                        }
                    }
                });
            });

            ui.add_space(10.0);

            // 搜尋欄（只在需要時顯示）
            if self.show_osu_search_bar {
                ui.horizontal(|ui| {
                    if let Some(search_icon) = self.preloaded_icons.get("search.png") {
                        ui.image(egui::load::SizedTexture::new(
                            search_icon.id(),
                            egui::vec2(16.0, 16.0),
                        ));
                    }
                    ui.add_space(5.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.downloaded_maps_search)
                            .hint_text("搜尋圖譜...")
                            .desired_width(fixed_width - 50.0),
                    );
                    if response.changed() {
                        info!("搜尋關鍵字: {}", self.downloaded_maps_search);
                    }
                });
                ui.add_space(10.0);
            }

            // 圖譜列表
            egui::ScrollArea::vertical().show(ui, |ui| {
                let downloaded = get_downloaded_beatmaps(&self.download_directory);
                if downloaded.is_empty() {
                    ui.label("尚未下載任何圖譜");
                } else {
                    // 先收集所有符合搜尋條件的檔案
                    let search_term = self.downloaded_maps_search.to_lowercase();
                    let filtered_maps: Vec<_> = downloaded
                        .into_iter()
                        .filter(|file_name| {
                            search_term.is_empty()
                                || file_name.to_lowercase().contains(&search_term)
                        })
                        .collect();

                    for file_name in filtered_maps {
                        ui.horizontal(|ui| {
                            let is_expanded = self.expanded_map_indices.contains(&file_name);

                            // 展開/收起按鈕
                            if let Some(icon) = self.preloaded_icons.get(if is_expanded {
                                "expand_off.png"
                            } else {
                                "expand_on.png"
                            }) {
                                if ui
                                    .add(egui::ImageButton::new(egui::load::SizedTexture::new(
                                        icon.id(),
                                        egui::vec2(16.0, 16.0),
                                    )))
                                    .clicked()
                                {
                                    if is_expanded {
                                        self.expanded_map_indices.remove(&file_name);
                                    } else {
                                        self.expanded_map_indices.insert(file_name.clone());
                                    }
                                }
                            }

                            // 檔案名稱顯示
                            let available_width = fixed_width - 50.0;
                            let text = egui::RichText::new(&file_name).size(14.0);

                            egui::Frame::none().show(ui, |ui| {
                                ui.set_max_width(available_width);
                                ui.label(text).on_hover_text(&file_name);
                            });
                        });

                        // 如果展開，顯示操作按鈕
                        if self.expanded_map_indices.contains(&file_name) {
                            let file_name_clone = file_name.clone();
                            ui.horizontal(|ui| {
                                ui.add_space(20.0);

                                // 刪除按鈕
                                if let Some(delete_icon) = self.preloaded_icons.get("delete.png") {
                                    if ui
                                        .add(egui::ImageButton::new(egui::load::SizedTexture::new(
                                            delete_icon.id(),
                                            egui::vec2(16.0, 16.0),
                                        )))
                                        .clicked()
                                    {
                                        if let Err(e) = fs::remove_file(
                                            self.download_directory.join(&file_name),
                                        ) {
                                            error!("刪除檔案失敗: {}", e);
                                        }
                                    }
                                }

                                // 搜尋按鈕
                                if let Some(search_icon) = self.preloaded_icons.get("search.png") {
                                    if ui
                                        .add(egui::ImageButton::new(egui::load::SizedTexture::new(
                                            search_icon.id(),
                                            egui::vec2(16.0, 16.0),
                                        )))
                                        .clicked()
                                    {
                                        if let Some(id) = Self::extract_beatmap_id(&file_name_clone)
                                        {
                                            self.search_query =
                                                format!("https://osu.ppy.sh/beatmapsets/{}", id);
                                            self.perform_search(ui.ctx().clone());
                                        }
                                    }
                                }
                            });
                        }
                        ui.separator();
                    }
                }
            });
        });
    }

    // 新增一個輔助函數來從檔名提取 beatmap ID
    fn extract_beatmap_id(file_name: &str) -> Option<&str> {
        file_name.split(' ').find(|s| s.parse::<u32>().is_ok())
    }

    fn load_custom_background(
        &mut self,
        ctx: &egui::Context,
    ) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(path) = &self.custom_background_path {
            let image = image::ImageReader::open(path)?.decode()?;
            let size = [image.width() as _, image.height() as _];
            let image_buffer = image.to_rgba8();
            let pixels = image_buffer.as_flat_samples();
            let texture = ctx.load_texture(
                "custom_background",
                egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice()),
                egui::TextureOptions::default(),
            );
            self.custom_background = Some(texture);
            Ok(())
        } else {
            Err("No custom background path set".into())
        }
    }

    fn render_playlists(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                if ui.button("< 返回").clicked() {
                    self.show_playlists = false;
                }
                ui.heading("我的播放清單");
                
                // 新增搜尋按鈕
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(search_icon) = self.preloaded_icons.get("search.png") {
                        if ui.add(egui::ImageButton::new(
                            egui::load::SizedTexture::new(
                                search_icon.id(),
                                egui::vec2(16.0, 16.0),
                            ),
                        )).clicked() {
                            self.show_playlist_search_bar = !self.show_playlist_search_bar;
                        }
                    }
                });
            });
    
            ui.add_space(10.0);
    
            // 搜尋欄
            if self.show_playlist_search_bar {
                ui.horizontal(|ui| {
                    if let Some(search_icon) = self.preloaded_icons.get("search.png") {
                        ui.image(egui::load::SizedTexture::new(
                            search_icon.id(),
                            egui::vec2(16.0, 16.0),
                        ));
                    }
                    ui.add_space(5.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.playlist_search_query)
                            .hint_text("搜尋播放清單...")
                            .desired_width(ui.available_width() - 50.0)
                    );
                    if response.changed() {
                        info!("播放清單搜尋關鍵字: {}", self.playlist_search_query);
                    }
                });
                ui.add_space(10.0);
            }
    
            egui::ScrollArea::vertical().show(ui, |ui| {
                // Liked Songs 項目總是顯示
                self.render_liked_songs_item(ui);
                ui.add_space(5.0);
                ui.separator();
    
                // 過濾播放清單
                let playlists_clone = {
                    if let Ok(playlists) = self.spotify_user_playlists.lock() {
                        playlists.clone()
                    } else {
                        Vec::new()
                    }
                };
    
                let search_term = self.playlist_search_query.to_lowercase();
                let filtered_playlists = playlists_clone.into_iter().filter(|playlist| {
                    search_term.is_empty() || 
                    playlist.name.to_lowercase().contains(&search_term)
                });
    
                for playlist in filtered_playlists {
                    self.render_playlist_item(ui, &playlist);
                }
            });
        });
    }

    fn render_liked_songs_item(&mut self, ui: &mut egui::Ui) {
        ui.add_space(5.0);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 70.0), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::TRANSPARENT);

            let cover_size = egui::vec2(60.0, 60.0);
            let text_rect = rect.shrink2(egui::vec2(cover_size.x + 30.0, 0.0));

            ui.painter().text(
                text_rect.left_center() + egui::vec2(0.0, -10.0),
                egui::Align2::LEFT_CENTER,
                "Liked Songs",
                egui::FontId::proportional(18.0),
                ui.visuals().text_color(),
            );

            ui.painter().text(
                text_rect.left_center() + egui::vec2(0.0, 15.0),
                egui::Align2::LEFT_CENTER,
                "播放清單",
                egui::FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );

            let image_rect = egui::Rect::from_min_size(
                rect.left_center() - egui::vec2(0.0, cover_size.y / 2.0),
                cover_size,
            );

            ui.painter()
                .rect_filled(image_rect, 0.0, egui::Color32::GREEN);
            ui.painter().text(
                image_rect.center(),
                egui::Align2::CENTER_CENTER,
                "♥",
                egui::FontId::proportional(30.0),
                egui::Color32::WHITE,
            );
        }

        if response.clicked() {
            if self.spotify_liked_tracks.lock().unwrap().is_empty() {
                self.load_user_liked_tracks();
            }
            self.selected_playlist = None;
            self.show_liked_tracks = true;
            self.show_playlists = false;
            info!("切換到 Liked Songs 視圖");
        }
    }

    fn render_playlist_item(&mut self, ui: &mut egui::Ui, playlist: &SimplifiedPlaylist) {
        ui.add_space(5.0);

        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 70.0), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            ui.painter()
                .rect_filled(rect, 0.0, egui::Color32::TRANSPARENT);

            let cover_size = egui::vec2(60.0, 60.0);
            let text_rect = rect.shrink2(egui::vec2(cover_size.x + 30.0, 0.0));

            ui.painter().text(
                text_rect.left_center() + egui::vec2(0.0, -10.0),
                egui::Align2::LEFT_CENTER,
                &playlist.name,
                egui::FontId::proportional(18.0),
                ui.visuals().text_color(),
            );

            if let Some(owner) = &playlist.owner.display_name {
                ui.painter().text(
                    text_rect.left_center() + egui::vec2(0.0, 15.0),
                    egui::Align2::LEFT_CENTER,
                    owner,
                    egui::FontId::proportional(14.0),
                    ui.visuals().weak_text_color(),
                );
            }

            let image_rect = egui::Rect::from_min_size(
                rect.left_center() - egui::vec2(0.0, cover_size.y / 2.0),
                cover_size,
            );

            if let Some(cover_url) = playlist.images.first().map(|img| &img.url) {
                let texture = {
                    let mut textures = self.playlist_cover_textures.lock().unwrap();
                    if !textures.contains_key(cover_url) {
                        textures.insert(cover_url.clone(), None);
                        let ctx = ui.ctx().clone();
                        let url = cover_url.clone();
                        let textures_clone = self.playlist_cover_textures.clone();
                        tokio::spawn(async move {
                            if let Ok(texture) =
                                Self::load_texture_async(&ctx, &url, Duration::from_secs(30)).await
                            {
                                let mut textures = textures_clone.lock().unwrap();
                                textures.insert(url, Some(texture));
                                ctx.request_repaint();
                            }
                        });
                    }
                    textures.get(cover_url).and_then(|t| t.clone())
                };

                if let Some(texture) = texture {
                    ui.painter().image(
                        texture.id(),
                        image_rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                } else {
                    ui.painter()
                        .rect_filled(image_rect, 0.0, ui.visuals().faint_bg_color);
                    ui.painter().text(
                        image_rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "加載中",
                        egui::FontId::proportional(14.0),
                        ui.visuals().text_color(),
                    );
                }
            } else {
                ui.painter()
                    .rect_filled(image_rect, 0.0, ui.visuals().faint_bg_color);
                ui.painter().text(
                    image_rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "",
                    egui::FontId::proportional(14.0),
                    ui.visuals().text_color(),
                );
            }
        }

        if response.clicked() {
            self.selected_playlist = Some(playlist.clone());
            self.load_playlist_tracks(playlist.id.clone());
            self.show_liked_tracks = false;
            self.show_playlists = false; // 確保關閉播放清單列表視圖
            info!("正在加載播放清單: {}", playlist.name);
        }
    }
    fn render_playlist_content(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            // 頂部標題列
            ui.horizontal(|ui| {
                if ui.button("< 返回").clicked() {
                    self.selected_playlist = None;
                    self.show_liked_tracks = false;
                    self.show_playlists = true;
                }

                let available_width = ui.available_width();
                let mut title = if self.show_liked_tracks {
                    "Liked Songs".to_string()
                } else if let Some(playlist) = &self.selected_playlist {
                    playlist.name.clone()
                } else {
                    "".to_string()
                };

                // 動態調整標題大小或截斷
                let mut font_size = 24.0;
                while ui.fonts(|f| {
                    f.layout_no_wrap(
                        title.clone(),
                        egui::FontId::new(font_size, egui::FontFamily::Proportional),
                        egui::Color32::WHITE,
                    )
                }).size().x > available_width - 150.0 // 為搜尋按鈕預留更多空間
                {
                    font_size -= 1.0;
                    if font_size < 16.0 {
                        while ui.fonts(|f| {
                            f.layout_no_wrap(
                                title.clone(),
                                egui::FontId::new(16.0, egui::FontFamily::Proportional),
                                egui::Color32::WHITE,
                            )
                        }).size().x > available_width - 150.0
                        {
                            if title.chars().count() > 3 {
                                title.pop();
                            } else {
                                break;
                            }
                        }
                        title.push_str("...");
                        font_size = 16.0;
                        break;
                    }
                }

                ui.heading(egui::RichText::new(title).size(font_size));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("🔄 重新加載").clicked() {
                        if self.show_liked_tracks {
                            self.load_user_liked_tracks();
                        } else if let Some(playlist) = &self.selected_playlist {
                            self.load_playlist_tracks(playlist.id.clone());
                        }

                        // 觸發更新檢查
                        let spotify_client = self.spotify_client.clone();
                        let liked_songs_cache = self.liked_songs_cache.clone();
                        let sender = self.update_check_sender.clone();

                        tokio::spawn(async move {
                            let spotify = spotify_client.lock().unwrap().clone();

                            if let Some(spotify) = spotify {
                                let cache_path = {
                                    let cache = liked_songs_cache.lock().unwrap();
                                    cache
                                        .as_ref()
                                        .map(|c| PathBuf::from(&format!("{:?}", c.last_updated)))
                                };

                                if let Some(path) = cache_path {
                                    match Self::check_for_updates(&spotify, &path).await {
                                        Ok(has_updates) => {
                                            if let Err(e) = sender.send(has_updates).await {
                                                error!("發送更新檢查結果時發生錯誤: {:?}", e);
                                            }
                                        }
                                        Err(e) => {
                                            error!("檢查更新時發生錯誤: {:?}", e);
                                        }
                                    }
                                } else {
                                    error!("無法獲取緩存路徑");
                                }
                            }
                        });
                    }

                    // 搜尋按鈕
                    if let Some(search_icon) = self.preloaded_icons.get("search.png") {
                        if ui.add(egui::ImageButton::new(
                            egui::load::SizedTexture::new(
                                search_icon.id(),
                                egui::vec2(16.0, 16.0),
                            ),
                        )).clicked() {
                            self.show_tracks_search_bar = !self.show_tracks_search_bar;
                        }
                    }
                });
            });

            // 搜尋欄
            if self.show_tracks_search_bar {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if let Some(search_icon) = self.preloaded_icons.get("search.png") {
                        ui.image(egui::load::SizedTexture::new(
                            search_icon.id(),
                            egui::vec2(16.0, 16.0),
                        ));
                    }
                    ui.add_space(5.0);
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.tracks_search_query)
                            .hint_text("搜尋歌曲...")
                            .desired_width(ui.available_width() - 50.0)
                    );
                    if response.changed() {
                        info!("歌曲搜尋關鍵字: {}", self.tracks_search_query);
                    }
                });
            }

            // 處理更新檢查結果
            while let Ok(has_updates) = self.update_check_receiver.try_recv() {
                if has_updates {
                    info!("發現更新，正在重新加載...");
                    ui.label("發現更新，正在重新加載...");
                    if self.show_liked_tracks {
                        self.load_user_liked_tracks();
                    } else if let Some(playlist) = &self.selected_playlist {
                        self.load_playlist_tracks(playlist.id.clone());
                    }
                } else {
                    info!("沒有發現更新，使用緩存數據");
                    ui.label("沒有發現更新，使用緩存數據");
                }
            }

            ui.add_space(10.0);

            let is_loading = self.is_searching.load(Ordering::SeqCst);
            let tracks = if self.show_liked_tracks {
                self.spotify_liked_tracks.lock().unwrap().clone()
            } else {
                self.spotify_playlist_tracks.lock().unwrap().clone()
            };

            if is_loading {
                ui.add_space(20.0);
                ui.add(egui::Spinner::new().size(32.0));
                ui.label("正在加載...");
            } else if tracks.is_empty() {
                ui.add_space(20.0);
                ui.label("沒有找到曲目");
            } else {
                // 過濾歌曲
                let search_term = self.tracks_search_query.to_lowercase();
                let filtered_tracks: Vec<_> = tracks
                    .iter()
                    .enumerate()
                    .filter(|(_, track)| {
                        search_term.is_empty() ||
                        track.name.to_lowercase().contains(&search_term) ||
                        track.artists.iter().any(|artist| 
                            artist.name.to_lowercase().contains(&search_term)
                        )
                    })
                    .collect();

                egui::ScrollArea::vertical().show_rows(
                    ui,
                    40.0,
                    filtered_tracks.len(),
                    |ui, row_range| {
                        for i in row_range {
                            if let Some((original_index, track)) = filtered_tracks.get(i) {
                                self.render_track_item(ui, track, *original_index);
                            }
                        }
                    },
                );
            }
        });
    }

    fn render_track_item(&mut self, ui: &mut egui::Ui, track: &FullTrack, index: usize) {
        ui.add_space(5.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(egui::RichText::new(format!("{}.", index + 1)).size(18.0))
                    .wrap(false),
            );
            ui.add_space(10.0);
    
            let content_width = ui.available_width() - 40.0;
    
            ui.vertical(|ui| {
                ui.set_width(content_width);
    
                // 歌曲名稱
                let title = track.name.clone();
                ui.label(egui::RichText::new(title).size(18.0).strong());
    
                // 歌手名稱
                let artists = track
                    .artists
                    .iter()
                    .map(|a| a.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.label(egui::RichText::new(artists).size(16.0).weak());
            });
    
            // 搜尋按鈕
            if let Some(search_icon) = self.preloaded_icons.get("search.png") {
                let response = ui.add(egui::ImageButton::new(
                    egui::load::SizedTexture::new(
                        search_icon.id(),
                        egui::vec2(16.0, 16.0),
                    ),
                ));
    
                if response.clicked() {
                    if let Some(spotify_url) = track.external_urls.get("spotify") {
                        self.search_query = spotify_url.clone();
                    } else {
                        self.search_query = format!(
                            "{} {}",
                            track.name,
                            track
                                .artists
                                .iter()
                                .map(|a| a.name.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        );
                    }
                    let ctx = ui.ctx().clone();
                    self.perform_search(ctx);
                }
    
                response.on_hover_text("以此搜尋");
            }
        });
        ui.add_space(5.0);
        ui.separator();
    }

    fn load_user_playlists(&self) {
        let spotify_client = self.spotify_client.clone();
        let user_playlists = self.spotify_user_playlists.clone();
        let ctx = self.ctx.clone();
        let cache_path = get_app_data_path().join("playlists_cache.json");

        tokio::spawn(async move {
            match get_user_playlists(spotify_client).await {
                Ok(playlists) => {
                    *user_playlists.lock().unwrap() = playlists.clone();
                    // 將播放列表緩存保存到文件
                    if let Err(e) =
                        fs::write(&cache_path, serde_json::to_string(&playlists).unwrap())
                    {
                        error!("保存播放列表緩存失敗: {:?}", e);
                    }
                    ctx.request_repaint();
                }
                Err(e) => {
                    error!("獲取用戶播放清單失敗: {:?}", e);
                }
            }
        });
    }

    fn load_playlist_tracks(&self, playlist_id: PlaylistId) {
        let spotify_client = self.spotify_client.clone();
        let playlist_tracks = self.spotify_playlist_tracks.clone();
        let ctx = self.ctx.clone();
        let is_searching = self.is_searching.clone();
        let playlist_id_string = playlist_id.id().to_string();
        let cache_ttl = self.cache_ttl;
        let update_check_result = self.update_check_result.clone();
        let cache_path =
            get_app_data_path().join(format!("playlist_{}_cache.json", playlist_id_string));

        tokio::spawn(async move {
            is_searching.store(true, Ordering::SeqCst);

            let should_update = if let Ok(metadata) = fs::metadata(&cache_path) {
                metadata.modified().unwrap().elapsed().unwrap() > cache_ttl
            } else {
                true
            };

            // 檢查是否有更新
            let has_updates = {
                let spotify_option = spotify_client.lock().unwrap().clone();
                if let Some(spotify) = spotify_option {
                    match Self::check_for_updates(&spotify, &cache_path).await {
                        Ok(updates) => updates,
                        Err(e) => {
                            error!("檢查更新時發生錯誤: {:?}", e);
                            false
                        }
                    }
                } else {
                    false
                }
            };

            if should_update || has_updates {
                info!("正在更新播放列表 {} 的緩存", playlist_id_string);

                match get_playlist_tracks(spotify_client.clone(), playlist_id_string.clone()).await
                {
                    Ok(tracks) => {
                        let tracks_len = tracks.len();
                        *playlist_tracks.lock().unwrap() = tracks.clone();
                        let cache = PlaylistCache {
                            tracks,
                            last_updated: SystemTime::now(),
                        };
                        if let Err(e) =
                            fs::write(&cache_path, serde_json::to_string(&cache).unwrap())
                        {
                            error!("保存播放列表緩存失敗: {:?}", e);
                        }
                        info!(
                            "成功更新緩存並加載 {} 首曲目，播放列表 ID: {}",
                            tracks_len, playlist_id_string
                        );
                    }
                    Err(e) => {
                        error!("獲取播放列表 {} 曲目失敗: {:?}", playlist_id_string, e);
                    }
                }
            } else {
                if let Ok(cached_data) = fs::read_to_string(&cache_path) {
                    if let Ok(cached) = serde_json::from_str::<PlaylistCache>(&cached_data) {
                        *playlist_tracks.lock().unwrap() = cached.tracks;
                        info!(
                            "使用緩存的播放列表曲目，播放列表 ID: {}, 曲目數量: {}",
                            playlist_id_string,
                            playlist_tracks.lock().unwrap().len()
                        );
                    }
                }
            }

            *update_check_result.lock().unwrap() = None;
            is_searching.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    fn load_user_liked_tracks(&self) {
        let spotify_client = self.spotify_client.clone();
        let liked_tracks = self.spotify_liked_tracks.clone();
        let is_searching = self.is_searching.clone();
        let ctx = self.ctx.clone();
        let cache_ttl = self.cache_ttl;
        let update_check_result = self.update_check_result.clone();
        let cache_path = get_app_data_path().join("liked_tracks_cache.json");

        tokio::spawn(async move {
            is_searching.store(true, Ordering::SeqCst);

            let should_update = if let Ok(metadata) = fs::metadata(&cache_path) {
                metadata.modified().unwrap().elapsed().unwrap() > cache_ttl
            } else {
                true
            };

            // 檢查是否有更新
            let has_updates = {
                let spotify_option = spotify_client.lock().unwrap().clone();
                if let Some(spotify) = spotify_option {
                    match Self::check_for_updates(&spotify, &cache_path).await {
                        Ok(updates) => updates,
                        Err(e) => {
                            error!("檢查更新時發生錯誤: {:?}", e);
                            false
                        }
                    }
                } else {
                    false
                }
            };

            if should_update || has_updates {
                info!("正在更新喜歡的曲目緩存");
                let mut all_tracks = Vec::new();
                let spotify_option = spotify_client.lock().unwrap().clone();

                if let Some(spotify) = spotify_option {
                    let mut offset = 0;
                    loop {
                        match spotify
                            .current_user_saved_tracks_manual(None, Some(50), Some(offset))
                            .await
                        {
                            Ok(page) => {
                                let page_items_len = page.items.len();
                                all_tracks.extend(
                                    page.items.into_iter().map(|saved_track| saved_track.track),
                                );

                                if page.next.is_none() {
                                    break;
                                }
                                offset += page_items_len as u32;
                            }
                            Err(e) => {
                                error!("獲取用戶喜歡的曲目失敗: {:?}", e);
                                break;
                            }
                        }
                    }

                    *liked_tracks.lock().unwrap() = all_tracks.clone();
                    let cache = PlaylistCache {
                        tracks: all_tracks.clone(),
                        last_updated: SystemTime::now(),
                    };
                    if let Err(e) = fs::write(&cache_path, serde_json::to_string(&cache).unwrap()) {
                        error!("保存喜歡的曲目緩存失敗: {:?}", e);
                    }

                    info!("成功更新緩存並加載 {} 首喜歡的曲目", all_tracks.len());
                } else {
                    error!("Spotify 客戶端未初始化");
                }
            } else {
                if let Ok(cached_data) = fs::read_to_string(&cache_path) {
                    if let Ok(cached) = serde_json::from_str::<PlaylistCache>(&cached_data) {
                        *liked_tracks.lock().unwrap() = cached.tracks;
                        info!(
                            "使用緩存的喜歡的曲目，曲目數量: {}",
                            liked_tracks.lock().unwrap().len()
                        );
                    }
                }
            }

            *update_check_result.lock().unwrap() = None;
            is_searching.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    async fn check_for_updates(
        spotify: &AuthCodeSpotify,
        cache_path: &PathBuf,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let mut has_updates = false;

        if cache_path.file_name().unwrap() == "liked_tracks_cache.json" {
            // 檢查 Liked Songs 是否有更新
            let liked_songs = spotify
                .current_user_saved_tracks_manual(None, Some(1), Some(0))
                .await?;
            if let Ok(cached_data) = fs::read_to_string(cache_path) {
                if let Ok(cached) = serde_json::from_str::<PlaylistCache>(&cached_data) {
                    if liked_songs.total != cached.tracks.len() as u32 {
                        has_updates = true;
                        info!(
                            "Liked Songs 有更新: API 返回 {} 首歌曲，緩存中有 {} 首歌曲",
                            liked_songs.total,
                            cached.tracks.len()
                        );
                    } else {
                        info!(
                            "Liked Songs 沒有更新: API 返回 {} 首歌曲，緩存中有 {} 首歌曲",
                            liked_songs.total,
                            cached.tracks.len()
                        );
                    }
                }
            } else {
                info!("Liked Songs 緩存不存在");
                has_updates = true;
            }
        } else {
            // 檢查播放列表是否有更新
            let playlist_id = cache_path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .replace("playlist_", "")
                .replace("_cache", "");
            let playlist = spotify
                .playlist(PlaylistId::from_id(&playlist_id).unwrap(), None, None)
                .await?;
            if let Ok(cached_data) = fs::read_to_string(cache_path) {
                if let Ok(cached) = serde_json::from_str::<PlaylistCache>(&cached_data) {
                    if playlist.tracks.total != cached.tracks.len() as u32 {
                        has_updates = true;
                        info!(
                            "播放列表 {} 有更新: API 返回 {} 首歌曲，緩存中有 {} 首歌曲",
                            playlist.name,
                            playlist.tracks.total,
                            cached.tracks.len()
                        );
                    } else {
                        info!(
                            "播放列表 {} 沒有更新: API 返回 {} 首歌曲，緩存中有 {} 首歌曲",
                            playlist.name,
                            playlist.tracks.total,
                            cached.tracks.len()
                        );
                    }
                }
            } else {
                info!("播放列表 {} 緩存不存在", playlist.name);
                has_updates = true;
            }
        }

        info!(
            "檢查更新結果: {}",
            if has_updates {
                "有更新"
            } else {
                "沒有更新"
            }
        );
        Ok(has_updates)
    }

    //渲染正在播放的彈窗
    fn render_now_playing_popup(&mut self, ui: &mut egui::Ui, response: &egui::Response) {
        egui::popup::popup_below_widget(ui, egui::Id::new("now_playing_popup"), response, |ui| {
            ui.set_min_width(250.0);
            ui.set_max_width(300.0);

            let current_playing = self
                .currently_playing
                .lock()
                .ok()
                .and_then(|guard| guard.clone());

            match current_playing {
                Some(current_playing) => {
                    ui.horizontal(|ui| {
                        if let Some(spotify_icon) = &self.spotify_icon {
                            let size = egui::vec2(24.0, 24.0);
                            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                                spotify_icon.id(),
                                size,
                            )));
                        }
                        ui.label(egui::RichText::new("正在播放").strong());
                    });

                    ui.add_space(5.0);

                    ui.label(egui::RichText::new(&current_playing.track_info.name).size(16.0));
                    ui.label(egui::RichText::new(&current_playing.track_info.artists).size(14.0));

                    ui.add_space(10.0);

                    if ui.button("搜索此歌曲").clicked() {
                        if let Some(spotify_url) = &current_playing.spotify_url {
                            self.search_query = spotify_url.clone();
                        } else {
                            self.search_query = format!(
                                "{} {}",
                                current_playing.track_info.artists, current_playing.track_info.name
                            );
                        }
                        let ctx = ui.ctx().clone();
                        self.perform_search(ctx);
                        ui.close_menu();
                    }
                }
                None => {
                    ui.label("當前沒有正在播放的曲目");
                }
            }
        });
    }
    //渲染登錄用戶
    fn render_logged_in_user(&mut self, ui: &mut egui::Ui) {
        let avatar_size = egui::vec2(32.0, 32.0);
        let button_size = egui::vec2(40.0, 40.0); // 稍微增加按鈕大小，為頭像周圍留出一些空間

        let button = egui::Button::new("")
            .fill(egui::Color32::TRANSPARENT)
            .min_size(button_size)
            .frame(false);

        let response = ui.add(button);

        if ui.is_rect_visible(response.rect) {
            if let Some(avatar) = &*self.spotify_user_avatar.lock().unwrap() {
                let image_rect = egui::Rect::from_center_size(response.rect.center(), avatar_size);
                ui.painter().image(
                    avatar.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }

        // 動態調整藍色圈選範圍
        if response.hovered() {
            ui.painter().rect_stroke(
                response.rect,
                egui::Rounding::same(4.0),
                egui::Stroke::new(1.0, egui::Color32::LIGHT_BLUE),
            );
        }

        if response.clicked() {
            ui.memory_mut(|mem| mem.toggle_popup(egui::Id::new("auth_popup")));
        }

        self.render_auth_popup(ui, &response);
    }

    //渲染授權彈窗
    fn render_auth_popup(&mut self, ui: &mut egui::Ui, response: &egui::Response) {
        egui::popup::popup_below_widget(ui, egui::Id::new("auth_popup"), response, |ui| {
            ui.set_min_width(200.0);

            let user_name = match read_login_info() {
                Ok(login_infos) => login_infos
                    .get("spotify")
                    .and_then(|info| info.user_name.clone()),
                Err(_) => None,
            };

            // Spotify 授權部分
            if self.spotify_authorized.load(Ordering::SeqCst) {
                let button_text = if let Some(name) = &user_name {
                    format!("{} (登出)", name)
                } else {
                    "Spotify (登出)".to_string()
                };
                if self
                    .create_auth_button(ui, &button_text, "spotify_icon_black.png")
                    .clicked()
                {
                    self.logout_spotify();
                    ui.close_menu();
                }
            } else {
                // 未登入時的授權邏輯保持不變
                let current_status = self.auth_manager.get_status(&AuthPlatform::Spotify);
                match current_status {
                    AuthStatus::NotStarted | AuthStatus::Failed(_) => {
                        if self
                            .create_auth_button(ui, "Spotify 授權", "spotify_icon_black.png")
                            .clicked()
                        {
                            info!("Spotify 授權按鈕被點擊了！");
                            let ctx = ui.ctx().clone();
                            self.start_spotify_authorization(ctx);
                        }
                    }
                    AuthStatus::WaitingForBrowser
                    | AuthStatus::Processing
                    | AuthStatus::TokenObtained => {
                        let button = egui::Button::new(egui::RichText::new("授權中...").size(16.0))
                            .min_size(egui::vec2(200.0, 40.0));
                        let response = ui.add(button);

                        if response.clicked() {
                            self.cancel_authorization();
                        }

                        if let Some(start_time) = self.auth_start_time {
                            let elapsed = start_time.elapsed();
                            let remaining = Duration::from_secs(30)
                                .checked_sub(elapsed)
                                .unwrap_or_default();
                            if remaining.as_secs() > 0 {
                                let progress = 1.0 - (remaining.as_secs() as f32 / 30.0);
                                let rect = response.rect;
                                let progress_rect = egui::Rect::from_min_size(
                                    rect.min,
                                    egui::vec2(rect.width() * progress, rect.height()),
                                );
                                ui.painter().rect_filled(
                                    progress_rect,
                                    0.0,
                                    egui::Color32::from_rgba_premultiplied(0, 255, 0, 100),
                                );
                                ui.painter().text(
                                    rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    format!("授權中...{}秒", remaining.as_secs()),
                                    egui::FontId::default(),
                                    egui::Color32::BLACK,
                                );
                            } else {
                                self.cancel_authorization();
                            }
                        }
                    }
                    AuthStatus::Completed => {
                        ui.label("Spotify 授權成功！");
                        if ui.button("關閉").clicked() {
                            ui.close_menu();
                        }
                    }
                }
            }

            ui.add_space(5.0);

            // Osu 授權部分
            if self
                .create_auth_button(ui, "建構中", "osu!logo.png")
                .clicked()
            {
                info!("Osu 授權按鈕被點擊了！");
                // TODO: 實現 Osu 授權邏輯
                ui.close_menu();
            }
        });
    }

    fn logout_spotify(&mut self) {
        info!("用戶登出 Spotify");
        self.spotify_authorized.store(false, Ordering::SeqCst);
        *self.spotify_user_avatar.lock().unwrap() = None;
        *self.spotify_user_name.lock().unwrap() = None;
        *self.spotify_user_avatar_url.lock().unwrap() = None;
        self.need_reload_avatar.store(true, Ordering::SeqCst);
        self.show_spotify_now_playing = false;
        self.should_detect_now_playing
            .store(false, Ordering::SeqCst);
        *self.currently_playing.lock().unwrap() = None;
        self.spotify_track_liked_status.lock().unwrap().clear();

        // 重置 Spotify 客戶端
        if let Ok(mut spotify_client) = self.spotify_client.try_lock() {
            *spotify_client = None;
        }

        // 重置授權管理器
        self.auth_manager.reset(&AuthPlatform::Spotify);
        self.auth_start_time = None;
        self.auth_in_progress.store(false, Ordering::SeqCst);
        self.show_auth_progress = false;

        // 刪除 login_info.json 文件
        let file_path = get_app_data_path().join("login_info.json");
        if let Err(e) = std::fs::remove_file(file_path) {
            error!("刪除 login_info.json 失敗: {}", e);
        }
        // 刪除使用者頭像
        if let Some(user_name) = self.spotify_user_name.lock().unwrap().as_ref() {
            let avatar_path = Self::get_avatar_path(user_name);
            if let Err(e) = std::fs::remove_file(avatar_path) {
                error!("刪除使用者頭像失敗: {}", e);
            }
        }
    }

    fn render_guest_user(&mut self, ui: &mut egui::Ui) {
        let button = egui::Button::new("")
            .fill(egui::Color32::TRANSPARENT)
            .min_size(egui::vec2(100.0, 40.0))
            .frame(false);

        let response = ui.add(button);

        if ui.is_rect_visible(response.rect) {
            if let Some(default_avatar) = &self.default_avatar_texture {
                let image_rect = egui::Rect::from_min_size(
                    response.rect.right_top() + egui::vec2(-36.0, 4.0),
                    egui::vec2(32.0, 32.0),
                );
                ui.painter().image(
                    default_avatar.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }

            let text_pos = response.rect.left_center() + egui::vec2(5.0, 0.0);
            ui.painter().text(
                text_pos,
                egui::Align2::LEFT_CENTER,
                "Guest",
                egui::FontId::new(16.0, egui::FontFamily::Proportional),
                egui::Color32::BLACK,
            );
        }

        if response.hovered() {
            ui.painter().rect_stroke(
                response.rect,
                egui::Rounding::same(4.0),
                egui::Stroke::new(1.0, egui::Color32::LIGHT_BLUE),
            );
        }

        if response.clicked() {
            ui.memory_mut(|mem| mem.toggle_popup(egui::Id::new("auth_popup")));
        }

        self.render_auth_popup(ui, &response);
    }

    fn create_auth_button(&self, ui: &mut egui::Ui, text: &str, icon_path: &str) -> egui::Response {
        let button_padding = egui::vec2(8.0, 4.0);
        let icon_size = egui::vec2(24.0, 24.0);
        let spacing = 8.0;

        let total_width = 200.0;
        let total_height = 40.0;

        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(total_width, total_height), egui::Sense::click());

        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&response);

            // 繪製按鈕背景
            ui.painter()
                .rect_filled(rect, visuals.rounding, visuals.bg_fill);

            let mut content_rect = rect.shrink2(button_padding);

            // 繪製圖標（如果有）
            if let Some(texture) = self.preloaded_icons.get(icon_path) {
                let icon_rect = egui::Rect::from_min_size(content_rect.min, icon_size);
                ui.painter().image(
                    texture.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE, // 使用白色以確保圖標可見
                );
                content_rect.min.x += icon_size.x + spacing;
            }

            // 繪製文字
            let galley = ui.painter().layout_no_wrap(
                text.to_owned(),
                egui::FontId::new(16.0, egui::FontFamily::Proportional),
                visuals.text_color(),
            );
            let text_pos = content_rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0);
            ui.painter().galley(text_pos, galley, visuals.text_color());

            // 為了調試，繪製邊框
            ui.painter()
                .rect_stroke(rect, visuals.rounding, visuals.fg_stroke);
        }

        response
    }

    fn load_icon(ctx: &egui::Context, icon_path: &str) -> Option<egui::TextureHandle> {
        let icon_bytes: &[u8] = match icon_path {
            "spotify_icon_black.png" => {
                info!("嘗試加載 Spotify 圖標");
                include_bytes!("assets/spotify_icon_black.png")
            }
            "osu!logo.png" => {
                info!("嘗試加載 Osu 圖標");
                include_bytes!("assets/osu!logo.png")
            }
            "Spotify_Full_Logo_RGB_White.png" => {
                info!("嘗試加載 Spotify 完整 White logo");
                include_bytes!("assets/Spotify_Full_Logo_RGB_White.png")
            }
            "Spotify_Full_Logo_RGB_Black.png" => {
                info!("嘗試加載 Spotify 完整 Black logo");
                include_bytes!("assets/Spotify_Full_Logo_RGB_Black.png")
            }
            "osu!logo@2x.png" => {
                info!("嘗試加載 osu!logo@2x.png");
                include_bytes!("assets/osu!logo@2x.png")
            }
            "search.png" => {
                info!("嘗試加載 search.png");
                include_bytes!("assets/search.png")
            }
            "like.png" => {
                info!("嘗試加載 like.png");
                include_bytes!("assets/like.png")
            }
            "liked.png" => {
                info!("嘗試加載 liked.png");
                include_bytes!("assets/liked.png")
            }
            "expand_on.png" => {
                info!("嘗試加載 expand_on.png");
                include_bytes!("assets/expand_on.png")
            }
            "expand_off.png" => {
                info!("嘗試加載 expand_off.png");
                include_bytes!("assets/expand_off.png")
            }
            "play.png" => {
                info!("嘗試加載 play.png");
                include_bytes!("assets/play.png")
            }
            "pause.png" => {
                info!("嘗試加載 pause.png");
                include_bytes!("assets/pause.png")
            }
            "download.png" => {
                info!("嘗試加載 download.png");
                include_bytes!("assets/download.png")
            }
            "delete.png" => {
                info!("嘗試加載 delete.png");
                include_bytes!("assets/delete.png")
            }
            "downloading.png" => {
                info!("嘗試加載 downloading.png");
                include_bytes!("assets/downloading.png")
            }
            "background1.jpg" => {
                info!("嘗試加載 background.jpg");
                include_bytes!("assets/background1.jpg")
            }
            "background_light2.jpg" => {
                info!("嘗試加載 background_light2.jpg");
                include_bytes!("assets/background_light2.jpg")
            }
            _ => {
                error!("未知的圖標路徑: {}", icon_path);
                return None;
            }
        };

        match image::load_from_memory(icon_bytes) {
            Ok(image) => {
                let image = image.to_rgba8();
                let size = [image.width() as _, image.height() as _];
                let pixels = image.as_flat_samples();
                let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

                info!("成功加載圖標: {}", icon_path);
                Some(ctx.load_texture(icon_path, color_image, egui::TextureOptions::default()))
            }
            Err(e) => {
                error!("無法加載圖標 {}: {:?}", icon_path, e);
                None
            }
        }
    }

    // 渲染中央面板
    fn render_central_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let available_rect = ui.max_rect();

            // 選擇背景圖片
            let background_image = if let Some(custom_bg) = &self.custom_background {
                custom_bg.clone()
            } else {
                // 使用預設背景的邏輯保持不變
                if ui.visuals().dark_mode {
                    self.last_background_key = "background1.jpg".to_string();
                } else {
                    self.last_background_key = "background_light2.jpg".to_string();
                }

                self.preloaded_icons
                    .get(&self.last_background_key)
                    .cloned()
                    .unwrap_or_else(|| {
                        Self::load_icon(ctx, &self.last_background_key).unwrap_or_else(|| {
                            error!("無法加載背景圖片");
                            ctx.load_texture(
                                "background_fallback",
                                egui::ColorImage::example(),
                                Default::default(),
                            )
                        })
                    })
            };

            // 渲染背景圖片
            ui.painter().image(
                background_image.id(),
                available_rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 180),
            );

            // 根據主題選擇遮罩顏色
            let mask_color = if ui.visuals().dark_mode {
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 150) // 半透明黑色
            } else {
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 50) // 降低白色透明度
            };

            // 添加半透明遮罩
            ui.painter().rect_filled(available_rect, 0.0, mask_color);

            // 在背景上方渲染其他內容
            egui::Frame::none()
                .fill(egui::Color32::TRANSPARENT) // 使用透明背景
                .show(ui, |ui| {
                    let content_rect = ui.max_rect();
                    let window_size = content_rect.size();

                    self.update_font_size(ui);
                    self.display_error_message(ui);

                    // 根據視窗大小決定佈局
                    if window_size.x >= 1000.0 {
                        self.render_large_window_layout(ui, window_size);
                    } else {
                        self.render_small_window_layout(ui, window_size);
                    }
                });
        });
    }

    fn render_large_window_layout(&mut self, ui: &mut egui::Ui, window_size: egui::Vec2) {
        ui.horizontal(|ui| {
            ui.add_space(25.0); // 左側增加25間距

            let content_width = window_size.x - 55.0; // 總寬度減去左右間距和中間間距
            let column_width = (content_width - 5.0) / 2.0; // 平均分配給兩列，中間保留5間距

            // Spotify 部分
            ui.vertical(|ui| {
                ui.set_min_width(column_width);
                ui.set_max_width(column_width);
                ui.set_min_height(window_size.y);
                ui.set_max_height(window_size.y);

                let frame = egui::Frame::none().inner_margin(egui::Margin::same(10.0));

                frame.show(ui, |ui| {
                    let mut spotify_scroll =
                        egui::ScrollArea::vertical().id_source("spotify_scroll");

                    if self.spotify_scroll_to_top {
                        spotify_scroll = spotify_scroll.scroll_offset(egui::vec2(0.0, 0.0));
                        self.spotify_scroll_to_top = false;
                        ui.ctx().request_repaint();
                    }

                    spotify_scroll.show(ui, |ui| {
                        self.display_spotify_results(ui, window_size);
                    });
                });
            });

            ui.add_space(5.0); // 中間增加5間距

            // osu! 部分
            ui.vertical(|ui| {
                ui.set_min_width(column_width);
                ui.set_max_width(column_width);
                ui.set_min_height(window_size.y);
                ui.set_max_height(window_size.y);

                let frame = egui::Frame::none().inner_margin(egui::Margin::same(10.0));

                frame.show(ui, |ui| {
                    let mut osu_scroll = egui::ScrollArea::vertical().id_source("osu_scroll");

                    if self.osu_scroll_to_top {
                        osu_scroll = osu_scroll.scroll_offset(egui::vec2(0.0, 0.0));
                        self.osu_scroll_to_top = false;
                        ui.ctx().request_repaint();
                    }

                    osu_scroll.show(ui, |ui| {
                        self.display_osu_results(ui, window_size);
                    });
                });
            });

            ui.add_space(25.0); // 右側增加25間距
        });
    }

    fn render_small_window_layout(&mut self, ui: &mut egui::Ui, window_size: egui::Vec2) {
        egui::ScrollArea::vertical()
            .id_source("small_window_scroll")
            .show(ui, |ui| {
                // Spotify 結果
                egui::CollapsingHeader::new(
                    egui::RichText::new("Spotify 結果").size(self.global_font_size * 1.1),
                )
                .default_open(true)
                .show(ui, |ui| {
                    if self.spotify_scroll_to_top {
                        ui.scroll_to_cursor(Some(egui::Align::TOP));
                        self.spotify_scroll_to_top = false;
                        ui.ctx().request_repaint();
                    }
                    self.display_spotify_results(ui, window_size);
                });

                // 添加一些間距
                ui.add_space(20.0);

                // Osu 結果
                egui::CollapsingHeader::new(
                    egui::RichText::new("osu! 結果").size(self.global_font_size * 1.1),
                )
                .default_open(true)
                .show(ui, |ui| {
                    if self.osu_scroll_to_top {
                        ui.scroll_to_cursor(Some(egui::Align::TOP));
                        self.osu_scroll_to_top = false;
                        ui.ctx().request_repaint();
                    }
                    self.display_osu_results(ui, window_size);
                });
            });
    }

    fn render_search_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let available_width = ui.available_width();
        let button_width = 30.0;
        let spacing = 5.0;
        let text_edit_width = available_width - 2.0 * button_width - 2.0 * spacing;
        let text_edit_height = 32.0;

        let search_bar_id = egui::Id::new("search_bar");

        let frame = egui::Frame::none()
            .fill(ui.visuals().extreme_bg_color)
            .inner_margin(egui::Margin::same(4.0))
            .rounding(egui::Rounding::same(2.0));

        frame.show(ui, |ui| {
            ui.style_mut().spacing.item_spacing.x = spacing;

            ui.horizontal(|ui| {
                let text_edit = egui::TextEdit::singleline(&mut self.search_query)
                    .id(search_bar_id)
                    .font(egui::FontId::proportional(16.0))
                    .margin(egui::vec2(5.0, 0.0))
                    .desired_width(text_edit_width)
                    .vertical_align(egui::Align::Center)
                    .cursor_at_end(false);

                let response =
                    ui.add_sized(egui::vec2(text_edit_width, text_edit_height), text_edit);

                if response.changed() {
                    ctx.request_repaint();
                }

                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.perform_search(ctx.clone());
                }

                if !self.search_query.is_empty() {
                    if ui
                        .add_sized([button_width, text_edit_height], egui::Button::new("✖"))
                        .clicked()
                    {
                        self.search_query.clear();
                        ui.memory_mut(|mem| mem.request_focus(search_bar_id));
                    }
                }

                if ui
                    .add_sized([button_width, text_edit_height], egui::Button::new("🔍"))
                    .clicked()
                {
                    self.perform_search(ctx.clone());
                }
            });
        });
    }

    fn update_font_size(&mut self, ui: &mut egui::Ui) {
        if ui
            .memory_mut(|mem| mem.data.get_temp::<f32>(egui::Id::new("global_font_size")))
            .map_or(true, |old_size| old_size != self.global_font_size)
        {
            ui.memory_mut(|mem| {
                mem.data
                    .insert_temp(egui::Id::new("global_font_size"), self.global_font_size)
            });

            let text_style = egui::TextStyle::Body.resolve(ui.style());
            let mut new_text_style = text_style.clone();
            new_text_style.size = self.global_font_size;
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Body, new_text_style);
        }
    }

    fn update_all_sinks_volume(&self) {
        let volume = self.global_volume;
        let current_previews = self.current_previews.clone();

        tokio::spawn(async move {
            let previews = current_previews.lock().await;
            for (_, sink) in previews.iter() {
                sink.set_volume(volume);
            }
        });
    }

    fn display_error_message(&self, ui: &mut egui::Ui) {
        if let Ok(err_msg_guard) = self.err_msg.try_lock() {
            if !err_msg_guard.is_empty() {
                ui.label(format!("{}", *err_msg_guard));
            }
        }
    }

    async fn load_spotify_avatar(
        ctx: &egui::Context,
        url: &str,
        spotify_user_avatar: Arc<RwLock<Option<egui::TextureHandle>>>,
        need_reload_avatar: Arc<AtomicBool>,
    ) -> Result<(), anyhow::Error> {
        if need_reload_avatar.load(Ordering::SeqCst) {
            info!("開始加載 Spotify 用戶頭像: {}", url);
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                Self::retry_load_avatar(url, ctx, spotify_user_avatar.clone()),
            )
            .await
            .map_err(|_| anyhow::anyhow!("加載頭像超時"))?;

            match result {
                Ok(_) => {
                    info!("Spotify 用戶頭像加載成功");
                    need_reload_avatar.store(false, Ordering::SeqCst);
                    ctx.request_repaint();
                    Ok(())
                }
                Err(e) => {
                    error!("加載 Spotify 用戶頭像失敗: {:?}", e);
                    need_reload_avatar.store(false, Ordering::SeqCst);
                    Err(e)
                }
            }
        } else {
            Ok(())
        }
    }

    async fn retry_load_avatar(
        url: &str,
        ctx: &egui::Context,
        spotify_user_avatar: Arc<RwLock<Option<egui::TextureHandle>>>,
    ) -> Result<(), anyhow::Error> {
        let mut backoff: ExponentialBackoff<SystemClock> = ExponentialBackoff::default();
        loop {
            match Self::load_spotify_user_avatar(url, ctx).await {
                Ok(texture) => {
                    let mut avatar = spotify_user_avatar.write().await;
                    *avatar = Some(texture);
                    return Ok(());
                }
                Err(e) => {
                    if let Some(duration) = backoff.next_backoff() {
                        error!("加載頭像失敗，將在 {:?} 後重試: {:?}", duration, e);
                        tokio::time::sleep(duration).await;
                    } else {
                        return Err(anyhow::anyhow!("加載頭像失敗次數過多"));
                    }
                }
            }
        }
    }

    async fn load_spotify_user_avatar(
        url: &str,
        ctx: &egui::Context,
    ) -> Result<egui::TextureHandle, anyhow::Error> {
        info!("開始從 URL 加載 Spotify 用戶頭像: {}", url);
        let client = reqwest::Client::new();
        let response = client.get(url).send().await.context("獲取頭像數據失敗")?;
        let bytes = response.bytes().await.context("讀取頭像字節數據失敗")?;

        let image = tokio::task::spawn_blocking(move || image::load_from_memory(&bytes))
            .await?
            .context("解析圖像數據失敗")?;

        let size = [image.width() as _, image.height() as _];
        let image_buffer = image.to_rgba8();
        let pixels = image_buffer.as_flat_samples();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());

        let texture = ctx.load_texture(
            "spotify_user_avatar",
            color_image,
            egui::TextureOptions::default(),
        );

        info!("成功將頭像加載到 egui 上下文中");
        Ok(texture)
    }

    fn get_avatar_path(username: &str) -> PathBuf {
        get_app_data_path().join(format!("{}.jpg", username))
    }

    async fn download_and_save_avatar(url: &str, path: &PathBuf) -> Result<(), anyhow::Error> {
        let response = reqwest::get(url).await.context("下載頭像失敗")?;
        let bytes = response.bytes().await.context("讀取頭像數據失敗")?;
        tokio::fs::write(path, &bytes)
            .await
            .context("保存頭像失敗")?;
        Ok(())
    }

    fn check_and_update_avatar(&self, ctx: &egui::Context) {
        if let (Some(user_name), Some(avatar_url)) = (
            self.spotify_user_name.lock().unwrap().clone(),
            self.spotify_user_avatar_url.lock().unwrap().clone(),
        ) {
            let avatar_path = Self::get_avatar_path(&user_name);
            let last_update = self.last_avatar_update;
            let ctx_clone = ctx.clone();
            let spotify_user_avatar = self.spotify_user_avatar.clone();
            let need_reload_avatar = self.need_reload_avatar.clone();

            tokio::spawn(async move {
                if let Err(e) = Self::check_and_update_avatar_async(
                    &avatar_url,
                    &avatar_path,
                    last_update,
                    &ctx_clone,
                    spotify_user_avatar,
                    need_reload_avatar,
                )
                .await
                {
                    error!("檢查和更新頭像失敗: {:?}", e);
                }
            });
        }
    }

    async fn check_and_update_avatar_async(
        url: &str,
        path: &PathBuf,
        last_update: DateTime<Utc>,
        ctx: &egui::Context,
        spotify_user_avatar: Arc<Mutex<Option<egui::TextureHandle>>>,
        need_reload_avatar: Arc<AtomicBool>,
    ) -> Result<(), anyhow::Error> {
        if !path.exists() || last_update + chrono::Duration::hours(24) < Utc::now() {
            Self::download_and_save_avatar(url, path).await?;
            if let Some(texture) = Self::load_local_avatar(ctx, path)? {
                let mut avatar = spotify_user_avatar.lock().unwrap();
                *avatar = Some(texture);
                need_reload_avatar.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            }
        }
        Ok(())
    }

    fn load_local_avatar(
        ctx: &egui::Context,
        path: &PathBuf,
    ) -> Result<Option<egui::TextureHandle>, anyhow::Error> {
        let image = image::open(path).context("打開本地頭像文件失敗")?;
        let size = [image.width() as _, image.height() as _];
        let image_buffer = image.to_rgba8();
        let pixels = image_buffer.as_flat_samples();
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice());
        Ok(Some(ctx.load_texture(
            "spotify_user_avatar",
            color_image,
            egui::TextureOptions::default(),
        )))
    }
}

#[tokio::main]
async fn main() -> Result<(), AppError> {
    let app_data_path = get_app_data_path();
    fs::create_dir_all(&app_data_path).expect("無法創建應用程序數據目錄");
    // 初始化日誌
    let log_file = std::fs::File::create("output.log").context("Failed to create log file")?;
    let mut config_builder = simplelog::ConfigBuilder::new();
    if let Err(err) = config_builder.set_time_offset_to_local() {
        eprintln!("Failed to set local time offset: {:?}", err);
    }

    let debug_mode = env::var("DEBUG_MODE").unwrap_or_default() == "true"
        || env::args().any(|arg| arg == "--debug");

    let config = config_builder
        .set_target_level(LevelFilter::Error)
        .set_location_level(LevelFilter::Off)
        .set_thread_level(LevelFilter::Off)
        .set_level_padding(LevelPadding::Right)
        .build();
    WriteLogger::init(
        if debug_mode {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        },
        config,
        log_file,
    )
    .context("Failed to initialize logger")?;

    info!("Welcome");

    // 讀取配置
    let config_errors = Arc::new(Mutex::new(Vec::new()));

    // 初始化 HTTP 客戶端
    let client = Arc::new(tokio::sync::Mutex::new(Client::new()));
    let (sender, receiver) = tokio::sync::mpsc::channel(100);

    // 定義 cover_textures
    let cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>> =
        Arc::new(RwLock::new(HashMap::new()));
    let need_repaint = Arc::new(AtomicBool::new(false));

    // 檢查下載目錄
    if need_select_download_directory() {
        info!("需要選擇下載目錄");
        // 使用 rfd 庫來顯示目錄選擇對話框
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            if let Err(e) = save_download_directory(&path) {
                error!("保存下載目錄失敗: {:?}", e);
            } else {
                info!("已選擇並保存下載目錄: {:?}", path);
            }
        } else {
            error!("用戶未選擇下載目錄");
            // 可以在這裡添加錯誤處理邏輯,例如退出程序
            return Err(AppError::Other("未選擇下載目錄".to_string()));
        }
    }

    let download_dir = load_download_directory().expect("無法獲取下載目錄");
    info!("下載目錄: {:?}", download_dir);

    let mut native_options = eframe::NativeOptions::default();
    native_options.hardware_acceleration = eframe::HardwareAcceleration::Preferred;
    native_options.viewport = ViewportBuilder {
        title: Some(String::from("Search App")),
        inner_size: Some(egui::Vec2::new(730.0, 430.0)),
        min_inner_size: Some(egui::Vec2::new(730.0, 430.0)),
        resizable: Some(true),
        maximize_button: Some(true),
        transparent: Some(true),
        ..Default::default()
    };

    // 運行應用
    eframe::run_native(
        "Search App",
        native_options,
        Box::new(move |cc| {
            let ctx = cc.egui_ctx.clone();
            ctx.set_visuals(if dark_light::detect() == dark_light::Mode::Dark {
                egui::Visuals::dark()
            } else {
                egui::Visuals::light()
            });
            ctx.set_pixels_per_point(1.0);

            match SearchApp::new(
                client.clone(),
                sender,
                receiver,
                cover_textures.clone(),
                need_repaint.clone(),
                ctx,
                config_errors.clone(),
                debug_mode, // 新增: 傳遞下載目錄
            ) {
                Ok(app) => Box::new(app),
                Err(e) => {
                    eprintln!("Failed to create SearchApp: {}", e);
                    Box::new(ErrorApp::new(e.to_string()))
                }
            }
        }),
    )
    .map_err(|e| anyhow::anyhow!("Failed to run eframe: {}", e))?;

    Ok(())
}
struct ErrorApp {
    error: String,
    font_size: f32,
}

impl ErrorApp {
    fn new(error: String) -> Self {
        Self {
            error,
            font_size: 24.0,
        }
    }
}

impl eframe::App for ErrorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 設置字體
        let mut fonts = FontDefinitions::default();
        let font_data = include_bytes!("jf-openhuninn-2.0.ttf");

        fonts.font_data.insert(
            "jf-openhuninn".to_owned(),
            FontData::from_owned(font_data.to_vec()),
        );

        if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
            family.insert(0, "jf-openhuninn".to_owned());
        }
        if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
            family.insert(0, "jf-openhuninn".to_owned());
        }

        ctx.set_fonts(fonts);

        // 顯示錯誤訊息
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.heading(
                    egui::RichText::new("錯誤")
                        .size(self.font_size * 1.2)
                        .color(egui::Color32::RED),
                );
                ui.label(egui::RichText::new(&self.error).size(self.font_size));
            });
        });
    }
}
