/*use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_album_info, print_track_infos,
    search_album_by_name, search_album_by_url, search_track,
}; */
//上方為lib1裡的相關函數
// 引入所需模組
use lib::osu_search::{
    fetch_beatmapset_details, get_beatmapsets, get_osu_token, print_beatmap_info_gui, Beatmapset,
};
use lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, open_spotify_url, print_track_info_gui,
    search_track, Track,
};
use tokio;
//use tokio::runtime::Runtime;
//use ::egui::FontData;
use anyhow::Result;
use clipboard::{ClipboardContext, ClipboardProvider};
use eframe::{self, egui};
use egui::vec2;
use egui::viewport::ViewportBuilder;
use egui::{FontData, FontDefinitions, FontFamily};

use reqwest::Client;

use log::info;
use simplelog::*;

use std::default::Default;
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

// 定義 SpotifySearchApp結構，儲存程式狀態和數據
struct SpotifySearchApp {
    client: Arc<AsyncMutex<Client>>,
    access_token: Arc<AsyncMutex<String>>,
    search_query: String,
    search_results: Arc<AsyncMutex<Vec<Track>>>,
    osu_search_results: Arc<AsyncMutex<Vec<Beatmapset>>>,
    selected_beatmapset_details: Arc<AsyncMutex<Option<Beatmapset>>>,
    error_message: Arc<AsyncMutex<String>>,
    initialized: bool,
    is_searching: Arc<AtomicBool>,
    need_repaint: Arc<AtomicBool>,
    font_size: f32,
    relax_slider_value: i64,
    show_relax_window: bool,
    selected_beatmapset: Option<usize>,
}

//為上方實現Default trait，創建默認狀態
impl Default for SpotifySearchApp {
    fn default() -> Self {
        Self {
            client: Arc::new(AsyncMutex::new(Client::new())),
            access_token: Arc::new(AsyncMutex::new(String::new())),
            search_query: String::new(),
            search_results: Arc::new(AsyncMutex::new(Vec::new())),
            osu_search_results: Arc::new(AsyncMutex::new(Vec::new())),
            selected_beatmapset_details: Arc::new(AsyncMutex::new(None)),
            error_message: Arc::new(AsyncMutex::new(String::new())),
            initialized: false,
            is_searching: Arc::new(AtomicBool::new(false)),
            need_repaint: Arc::new(AtomicBool::new(false)),
            font_size: 14.0,
            show_relax_window: false,
            relax_slider_value: 0,
            selected_beatmapset: None,
        }
    }
}

impl eframe::App for SpotifySearchApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let window_size = ctx.input(|i| i.screen_rect.size()); // 当前窗口大小
        let base_window_size = egui::vec2(458.0, 323.0); // 基準視窗大小
        let base_font_size = 14.0; // 基準字體大小

        // 計算比例
        let scale_factor = window_size.x / base_window_size.x;
        self.font_size = base_font_size * scale_factor;

        // 請求更新介面，用於刷新GUI
        if self.need_repaint.load(Ordering::SeqCst) {
            ctx.request_repaint();
            self.need_repaint.store(false, Ordering::SeqCst);
        }

        // 初始化程式,和設置字體及獲取access token
        if !self.initialized {
            let client = self.client.clone();
            let access_token = self.access_token.clone();
            let error_message = self.error_message.clone();
            tokio::spawn(async move {
                let client_guard = client.lock().await;
                match get_access_token(&*client_guard).await {
                    Ok(token) => {
                        let mut access_token_guard = access_token.lock().await;
                        *access_token_guard = token;
                    }
                    Err(e) => {
                        let mut error_guard = error_message.lock().await;
                        *error_guard = format!("Failed to get access token: {}", e);
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

            self.initialized = true;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let text_style = egui::TextStyle::Body.resolve(ui.style());
            let mut new_text_style = text_style.clone();
            new_text_style.size = self.font_size;
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Body, new_text_style);
            ui.heading("Search for a song:");
            ui.horizontal(|ui| {
                let text_edit_width = ui.available_width() * 0.5;
                let text_edit_response = ui.add_sized(
                    egui::vec2(text_edit_width, 20.0 * self.font_size / base_font_size), // 調整高度以保持與基準字體大小的比例
                    egui::TextEdit::singleline(&mut self.search_query),
                );

                let cloned_response = text_edit_response.clone();

                // 檢測右鍵是否按下
                cloned_response.context_menu(|ui| {
                    if ui.button("Paste").clicked() {
                        let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
                        if let Ok(clipboard_contents) = ctx.get_contents() {
                            self.search_query = clipboard_contents;
                            ui.close_menu();
                        }
                    }
                    if ui.button("Relax").clicked() {
                        // 觸發浪費時間
                        self.show_relax_window = true;
                        ui.close_menu();
                    }
                });

                // 檢測Enter是否按下
                if text_edit_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.perform_search();
                }
            });

            ui.columns(2, |columns| {
                // 左邊顯示Spotify的結果
                columns[0].vertical(|ui| {
                    ui.heading("Spotify Results");
                    ui.push_id("spotify_results", |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            if let Ok(search_results_guard) = self.search_results.try_lock() {
                                if !search_results_guard.is_empty() {
                                    for track in search_results_guard.iter() {
                                        let (formatted_result, spotify_url, _track_name) =
                                        print_track_info_gui(track);

                                    // 顯示結果
                                    let response = ui.add(
                                        egui::Label::new(formatted_result.clone())
                                            .sense(egui::Sense::click_and_drag()),
                                    );

                                    // 雙擊
                                    if response.double_clicked() {
                                        if let Some(url) = &spotify_url {
                                            match open_spotify_url(url) {
                                                Ok(_) => {
                                                    //nothing
                                                }
                                                Err(e) => {
                                                    log::error!("Failed to open URL: {}", e);
                                                }
                                            }
                                        }
                                    }

                                    // 右鍵菜單
                                    response.context_menu(|ui| {
                                        if let Some(url) = &spotify_url {
                                            if ui.button("Copy URL").clicked() {
                                                let mut ctx: ClipboardContext =
                                                    ClipboardProvider::new().unwrap();
                                                ctx.set_contents(url.clone()).unwrap();
                                                ui.close_menu();
                                            }
                                            if ui.button("Open").clicked() {
                                                match open_spotify_url(url) {
                                                    Ok(_) => {
                                                        //nothing
                                                    }
                                                    Err(e) => {
                                                        log::error!(
                                                            "Failed to open URL: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                                ui.close_menu();
                                            }
                                        }
                                    });

                                    ui.add_space(10.0); // 間距
                                }
                            }
                        }
                    });
                });
            });

                            // 右邊顯示Osu的結果
                            columns[1].vertical(|ui| {
                                ui.heading("Osu Results");
                                ui.push_id("osu_results", |ui| {
                                    egui::ScrollArea::vertical().show(ui, |ui| {
                                        if let Ok(osu_search_results_guard) = self.osu_search_results.try_lock() {
                                            if !osu_search_results_guard.is_empty() {
                                                if let Some(selected_index) = self.selected_beatmapset {
                                                    let selected_beatmapset = &osu_search_results_guard[selected_index];
                                                    for beatmap in &selected_beatmapset.beatmaps {
                                                        let beatmap_info = format!(
                                                            "Difficulty: {}\nMode: {}\nStatus: {}\nLength: {} minutes\nVersion: {}",
                                                            beatmap.difficulty_rating,
                                                            beatmap.mode,
                                                            beatmap.status,
                                                            beatmap.total_length / 60,
                                                            beatmap.version
                                                        );
                                                        ui.label(beatmap_info);
                                                        ui.add_space(10.0);
                                                    }
                                                } else {
                                                    for (index, beatmapset) in osu_search_results_guard.iter().enumerate() {
                                                        if ui.button(format!("{} - {} (by {})", beatmapset.title, beatmapset.artist, beatmapset.creator)).clicked() {
                                                            self.selected_beatmapset = Some(index);
                                                        }
                                                        ui.separator();
                                                    }
                                                }
                                            }
                                        }
                                    });
                                });
                            });
                        });
                    });

        if self.show_relax_window {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading("Waste your time");

                    let slider = egui::Slider::new(&mut self.relax_slider_value, 0..=999_999_999)
                        .clamp_to_range(true)
                        .text("我不知道這是做啥");
                    ui.add_sized([ui.available_width(), 20.0], slider);

                    ui.label(format!("Value: {}", self.relax_slider_value));

                    if ui.button("Close").clicked() {
                        self.show_relax_window = false;
                    }
                });
            });
        }
    }
}

impl SpotifySearchApp {
    fn perform_search(&mut self) -> JoinHandle<Result<()>> {
        let client = self.client.clone();
        let query = self.search_query.clone();
        let search_results = self.search_results.clone();
        let osu_search_results = self.osu_search_results.clone();
        let is_searching = Arc::clone(&self.is_searching);
        let need_repaint = self.need_repaint.clone();
        let error_message = self.error_message.clone();

        // 記錄搜尋查詢
        log::info!("User searched for: {}", query);

        is_searching.store(true, Ordering::SeqCst);

        tokio::spawn(async move {
            let mut error = error_message.lock().await;
            error.clear();

            // 獲取 Spotify token
            let spotify_token = match get_access_token(&*client.lock().await).await {
                Ok(token) => token,
                Err(e) => {
                    let error_msg = format!("Error getting Spotify token: {:?}", e);
                    *error = error_msg.clone();
                    log::error!("{}", error_msg);
                    is_searching.store(false, Ordering::SeqCst);
                    need_repaint.store(true, Ordering::SeqCst);
                    return Err(anyhow::anyhow!(error_msg));
                }
            };

            // 獲取 Osu token
            let osu_token = match get_osu_token(&*client.lock().await).await {
                Ok(token) => token,
                Err(e) => {
                    let error_msg = format!("Error getting Osu token: {:?}", e);
                    *error = error_msg.clone();
                    log::error!("{}", error_msg);
                    is_searching.store(false, Ordering::SeqCst);
                    need_repaint.store(true, Ordering::SeqCst);
                    return Err(anyhow::anyhow!(error_msg));
                }
            };

            // Spotify search
            let spotify_result = if query.starts_with("http://") || query.starts_with("https://") {
                if query.starts_with("https://open.spotify") || query.starts_with("https://spotify")
                {
                    if is_valid_spotify_url(&query) {
                        let track_id = query
                            .split('/')
                            .last()
                            .unwrap_or("")
                            .split('?')
                            .next()
                            .unwrap_or("");
                        let track = get_track_info(&*client.lock().await, track_id, &spotify_token)
                            .await
                            .map_err(|e| anyhow::anyhow!("Error getting track info: {:?}", e))?;
                        Ok(vec![track])
                    } else {
                        let error_msg = "您似乎輸入了一個Spotify URL,但它是不正確的。";
                        *error = error_msg.to_string();
                        log::error!("{}", error_msg);
                        Err(anyhow::anyhow!(error_msg))
                    }
                } else {
                    let error_msg = "你疑似輸入URL,但它是不正確的。";
                    *error = error_msg.to_string();
                    log::error!("{}", error_msg);
                    Err(anyhow::anyhow!(error_msg))
                }
            } else {
                let (tracks, _) =
                    search_track(&*client.lock().await, &query, &spotify_token, 1, 20)
                        .await
                        .map_err(|e| anyhow::anyhow!("Error searching tracks: {:?}", e))?;
                Ok(tracks)
            };

            match spotify_result {
                Ok(tracks) => {
                    let mut results = search_results.lock().await;
                    *results = tracks;
                }
                Err(error) => {
                    log::error!("{}", error);
                    let mut error_msg = error_message.lock().await;
                    *error_msg = error.to_string();
                }
            }

            // Osu search
            let osu_result = get_beatmapsets(&*client.lock().await, &osu_token, &query).await;

            match osu_result {
                Ok(beatmapsets) => {
                    let mut results = osu_search_results.lock().await;
                    *results = beatmapsets;
                }
                Err(error) => {
                    log::error!("{}", error);
                    let mut error_msg = error_message.lock().await;
                    *error_msg = error.to_string();
                }
            }

            is_searching.store(false, Ordering::SeqCst);
            need_repaint.store(true, Ordering::SeqCst);

            Ok(())
        })
    }
}
fn main() {
    let log_file = File::create("output.log").unwrap();

    let mut config_builder = ConfigBuilder::new();
    let result = config_builder.set_time_offset_to_local();

    if let Err(err) = result {
        eprintln!("Failed to set local time offset: {:?}", err);
    }

    let config = config_builder
        .set_target_level(LevelFilter::Info)
        .set_level_padding(LevelPadding::Right)
        .build();

    WriteLogger::init(LevelFilter::Info, config, log_file).unwrap();

    info!("Welcome");

    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        let app = SpotifySearchApp::default();

        let viewport_builder = ViewportBuilder::default().with_inner_size(vec2(458.0, 323.0)); //預設窗口大小

        let native_options = eframe::NativeOptions {
            viewport: viewport_builder,
            ..Default::default()
        };
        eframe::run_native(
            "Search App",
            native_options,
            Box::new(move |cc| Box::new(app)),
        )
        .unwrap();
    });
}
