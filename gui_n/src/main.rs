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
use eframe;
use epi;
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
        }
    }
}
//定義GUI行為和邏輯
impl epi::App for SpotifySearchApp {
    fn name(&self) -> &str {       //程式名稱
        "Spotify Search App"
    }
    // 更新函數，處理GUI邏輯
    fn update(&mut self, ctx: &epi::egui::Context, _frame: &epi::Frame) {
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

            let font_data = include_bytes!("jf-openhuninn-2.0.ttf");
            let mut fonts = epi::egui::FontDefinitions::default();
            fonts.font_data.insert(
                "jf-openhuninn".to_owned(),
                epi::egui::FontData::from_owned(font_data.to_vec()),
            );

            fonts
                .families
                .get_mut(&epi::egui::FontFamily::Proportional)
                .unwrap()
                .insert(0, "jf-openhuninn".to_owned());
            fonts
                .families
                .get_mut(&epi::egui::FontFamily::Monospace)
                .unwrap()
                .insert(0, "jf-openhuninn".to_owned());

            ctx.set_fonts(fonts);

            self.initialized = true;
        }
        let window_size = ctx.input().screen_rect.size(); //擷取當前視窗大小

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Spotify Song Search");
            ui.horizontal(|ui| {
                ui.label("Search for a song:");
                let text_edit_response = ui.text_edit_singleline(&mut self.search_query);

                
                let cloned_response = text_edit_response.clone();

               //檢測右鍵是否按下
                cloned_response.context_menu(|ui| {
                    if ui.button("Paste").clicked() {
                        let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
                        if let Ok(clipboard_contents) = ctx.get_contents() {
                            self.search_query = clipboard_contents;
                            ui.close_menu();
                        }
                    }
                });

                // 檢測Enter是否按下
                if text_edit_response.lost_focus() && ui.input().key_pressed(egui::Key::Enter) {
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

            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Ok(search_results) = self.search_results.try_lock() {
                    if !search_results.is_empty() {
                        ui.label("Search Results:");
                        for track in search_results.iter() {
                            let (formatted_result, spotify_url) = print_track_info_gui(track);
                            ui.label(&formatted_result);

                            if let Some(url) = spotify_url {
                                // 創建右鍵菜單
                                ui.horizontal(|ui| {
                                    ui.hyperlink_to(url.clone(), url.clone()).context_menu(|ui| {
                                        if ui.button("Copy").clicked() {
                                            let mut ctx: ClipboardContext = ClipboardProvider::new().unwrap();
                                            ctx.set_contents(url.clone()).unwrap();
                                            ui.close_menu();
                                        }
                                        if ui.button("Open").clicked() {
                                            let spotify_uri = url.replace("https://open.spotify.com/", "spotify:");
                                            if let Err(_) = std::process::Command::new("spotify").arg(spotify_uri).spawn() {
                                                // 如果打開Spotify App失敗 則打開網頁版
                                                if webbrowser::open(&url).is_ok() {
                                                    ui.close_menu();
                                                }
                                            } else {
                                                ui.close_menu();
                                            }
                                        }
                                    });
                                });
                            }
                        
                            ui.add_space(10.0); // 間距
                        }
                    }
                }
            });
        });
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
                if query.starts_with("https://open.spotify")
                    || query.starts_with("https://spotify")
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
        let mut native_options = eframe::NativeOptions::default();
        native_options.initial_window_size = Some(egui::vec2(458.0, 323.0));  //目前視窗預設大小
        eframe::run_native(Box::new(app), native_options);
    });
}
