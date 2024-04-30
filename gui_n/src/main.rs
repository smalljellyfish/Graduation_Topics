/*use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_album_info, print_track_infos,
    search_album_by_name, search_album_by_url, search_track,
}; */
use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_track_info_gui, search_track,
    Track,
};
use tokio;
//use tokio::runtime::Runtime;
//use ::egui::FontData;
use eframe;
use epi;
use reqwest::Client;
use std::default::Default;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex as AsyncMutex;
use anyhow::{Result, Context};
use tokio::task::JoinHandle;

struct SpotifySearchApp {
    client: Arc<AsyncMutex<Client>>,
    access_token: Arc<AsyncMutex<String>>,
    search_query: String,
    search_results: Arc<AsyncMutex<Vec<Track>>>,
    error_message: String,
    initialized: bool,
    is_searching: Arc<AtomicBool>,
    need_repaint: Arc<AtomicBool>,
}
impl Default for SpotifySearchApp {
    fn default() -> Self {
        Self {
            client: Arc::new(AsyncMutex::new(Client::new())),
            access_token: Arc::new(AsyncMutex::new(String::new())),
            search_query: String::new(),
            search_results: Arc::new(AsyncMutex::new(Vec::new())),
            error_message: String::new(),
            initialized: false,
            is_searching: Arc::new(AtomicBool::new(false)),
            need_repaint: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl epi::App for SpotifySearchApp {
    fn name(&self) -> &str {
        "Spotify Search App"
    }

    fn update(&mut self, ctx: &epi::egui::Context, _frame: &epi::Frame) {
        if self.need_repaint.load(Ordering::SeqCst) {
            ctx.request_repaint();
            self.need_repaint.store(false, Ordering::SeqCst);
        }
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
                
                // 檢查Enter 鍵是否按下
                if text_edit_response.lost_focus() && ui.input().key_pressed(egui::Key::Enter) {
                    self.perform_search();
                }
            });
            if !self.error_message.is_empty() {
                
                egui::Window::new("Error")
                    .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO) 
                    .show(ui.ctx(), |ui| {
                        ui.colored_label(egui::Color32::RED, &self.error_message);
                    });
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
                                ui.hyperlink_to(url.clone(), &url);
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
        let error_message = Arc::new(AsyncMutex::new(self.error_message.clone()));

        is_searching.store(true, Ordering::SeqCst); 

        tokio::spawn(async move {
            if query.starts_with("http://") || query.starts_with("https://") {
                if is_valid_spotify_url(&query) {
                    let track_id = query.split('/').last().unwrap_or("").split('?').next().unwrap_or("");
                    let result = get_track_info(&*client.lock().await, track_id, &*access_token.lock().await).await
                        .context("Failed to get track info")?;
                    let mut results = search_results.lock().await;
                    *results = vec![result];
                } else {
                    let mut em = error_message.lock().await;
                    *em = "You seem to have entered a Spotify URL, but it is incorrect.".to_string();
                    search_results.lock().await.clear(); 
                }
            } else {
                let result = search_track(&*client.lock().await, &query, &*access_token.lock().await, 1, 20).await
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
        native_options.initial_window_size = Some(egui::vec2(458.0, 323.0));
        eframe::run_native(Box::new(app), native_options);
    });
}
