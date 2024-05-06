/*use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_album_info, print_track_infos,
    search_album_by_name, search_album_by_url, search_track,
}; */
//上方為lib1裡的相關函數

// 引入所需模組
use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_track_info_gui, search_track,
    Track,
};
use tokio;
//use tokio::runtime::Runtime;
//use ::egui::FontData;
use anyhow::{Context, Result};
use clipboard::{ClipboardContext, ClipboardProvider};
use eframe::{self, egui};
use egui::vec2;
use egui::viewport::ViewportBuilder;
use egui::{FontData, FontDefinitions, FontFamily};
use reqwest::Client;
use std::default::Default;
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
    error_message: Arc<AsyncMutex<String>>,
    initialized: bool,
    is_searching: Arc<AtomicBool>,
    need_repaint: Arc<AtomicBool>,
    font_size: f32,
    relax_slider_value: i64,
    show_relax_window: bool,
}
//為上方實現Default trait，創建默認狀態
impl Default for SpotifySearchApp {
    fn default() -> Self {
        Self {
            client: Arc::new(AsyncMutex::new(Client::new())),
            access_token: Arc::new(AsyncMutex::new(String::new())),
            search_query: String::new(),
            search_results: Arc::new(AsyncMutex::new(Vec::new())),
            error_message: Arc::new(AsyncMutex::new(String::new())),
            initialized: false,
            is_searching: Arc::new(AtomicBool::new(false)),
            need_repaint: Arc::new(AtomicBool::new(false)),
            font_size: 14.0,
            show_relax_window: false, 
            relax_slider_value: 0,
        }
    }
}
//定義GUI行為和邏輯
impl eframe::App for SpotifySearchApp {
    // 更新函數，處理GUI邏輯
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let window_size = ctx.input(|i| i.screen_rect.size()); // 当前窗口大小
        let base_window_size = egui::vec2(458.0, 323.0); // 基準視窗大小
        let base_font_size = 14.0; // 基準字體大小

        // 計算比例
        let scale_factor = window_size.x / base_window_size.x;
        self.font_size = base_font_size * scale_factor;

        //請求更新介面，用於刷新GUI
        if self.need_repaint.load(Ordering::SeqCst) {
            ctx.request_repaint();
            self.need_repaint.store(false, Ordering::SeqCst);
        }
        // 初始化程式,和設置字體及獲取access token
        if !self.initialized {
            let client = self.client.clone();
            let access_token = self.access_token.clone();
            tokio::spawn(async move {
                let client_guard = client.lock().await;
                let token = get_access_token(&*client_guard).await.unwrap();
                let mut access_token_guard = access_token.lock().await;
                *access_token_guard = token;
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
                //ui.label("Search for a song:");
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
            if let Ok(error_message_guard) = self.error_message.try_lock() {
                if !error_message_guard.is_empty() {
                    egui::Window::new("Error")
                        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                        .show(ctx, |ui| {
                            ui.colored_label(egui::Color32::LIGHT_BLUE, &*error_message_guard);
                        });
                }
            }
            if ui.button("Search").clicked() {
                self.perform_search();
            }
            if self.is_searching.load(Ordering::SeqCst) {
                ui.label("Searching...");
                ui.add(egui::ProgressBar::new(1.0));
                ctx.request_repaint();
            }
            ui.label(format!(
                "Window size: {:.0} x {:.0}",
                window_size.x, window_size.y
            ));

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Ok(search_results_guard) = self.search_results.try_lock() {
                        if !search_results_guard.is_empty() {
                            ui.label("Search Results:");
                            for track in search_results_guard.iter() {
                                let (formatted_result, spotify_url, _track_name) = print_track_info_gui(track);

                                // 顯示結果
                                let response = ui.add(egui::Label::new(formatted_result.clone()).sense(egui::Sense::click_and_drag()));

                                

                                // 雙擊
                                if response.double_clicked() {
                                    if let Some(url) = &spotify_url {
                                        //先打開Spotify APP
                                        let spotify_uri = url.replace("https://open.spotify.com/", "spotify:");
                                        if std::process::Command::new("spotify").arg(spotify_uri).spawn().is_err() {
                                            //若失敗則打開網頁
                                            if webbrowser::open(url).is_ok() {
                                                println!("Opened URL in browser: {}", url);
                                            }
                                        } else {
                                            println!("Opened URL in Spotify App: {}", url);
                                        }
                                    }
                                }

                                // 右键菜单
                                response.context_menu(|ui| {
                                    if let Some(url) = &spotify_url {
                                        if ui.button("Copy URL").clicked() {
                                            let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
                                            ctx.set_contents(url.clone()).unwrap();
                                            ui.close_menu();
                                        }
                                        if ui.button("Open in browser").clicked() {
                                            if webbrowser::open(url).is_ok() {
                                                ui.close_menu();
                                            }
                                        }
                                    }
                                });

                                ui.add_space(10.0); // 間距
                            }
                        }
                    }
                });
        });
        if self.show_relax_window {
            egui::CentralPanel::default().show(ctx, |ui| {
                //ui.set_min_size(ctx.input().screen_rect.size());  //用於設定最小視窗大小
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0); // space
                    ui.heading("Waste your time"); // title

                    ui.horizontal(|ui| {
                        let slider =
                            egui::Slider::new(&mut self.relax_slider_value, 0..=999_999_999)
                                .text("我不知道這是做啥");
                        ui.add_sized([ui.available_width(), 20.0], slider);
                    });

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
        let access_token = self.access_token.clone();
        let query = self.search_query.clone();
        let search_results = self.search_results.clone();
        let is_searching = Arc::clone(&self.is_searching);
        let need_repaint = self.need_repaint.clone();
        let error_message = self.error_message.clone();

        is_searching.store(true, Ordering::SeqCst);

        tokio::spawn(async move {
            // 清除之前的錯誤訊息
            let mut error = error_message.lock().await;
            error.clear();

            if query.starts_with("http://") || query.starts_with("https://") {
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
                        let result = get_track_info(
                            &*client.lock().await,
                            track_id,
                            &*access_token.lock().await,
                        )
                        .await
                        .context("Failed to get track info")?;
                        let mut results = search_results.lock().await;
                        *results = vec![result];
                    } else {
                        *error = "您似乎輸入了一個Spotify URL，但它是不正確的。".to_string();
                        search_results.lock().await.clear();
                    }
                } else {
                    *error = "你疑似輸入URL，但它是不正確的。".to_string();
                    search_results.lock().await.clear();
                }
            } else {
                let result = search_track(
                    &*client.lock().await,
                    &query,
                    &*access_token.lock().await,
                    1,
                    20,
                )
                .await
                .context("Failed to search tracks")?;
                let mut results = search_results.lock().await;
                *results = result.0;
            }

            is_searching.store(false, Ordering::SeqCst);
            need_repaint.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(async {
        let app = SpotifySearchApp::default();

        let viewport_builder = ViewportBuilder::default().with_inner_size(vec2(458.0, 323.0)); // 设置窗口大小

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
