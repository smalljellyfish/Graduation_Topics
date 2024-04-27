//use ::egui::debug_text::print;
/*use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_album_info, print_track_infos,
    search_album_by_name, search_album_by_url, search_track,
}; */
use spotify_search_lib::spotify_search::{
    get_access_token, print_track_info_gui, search_track,Track
};
use tokio;
use tokio::runtime::Runtime;
//use ::egui::FontData;
use eframe::{egui, epi};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::Mutex;

struct SpotifySearchApp {
    client: Arc<Mutex<Client>>,
    access_token: Arc<Mutex<String>>,
    search_query: String,
    search_results: Arc<Mutex<Vec<Track>>>
}
impl Default for SpotifySearchApp {
    fn default() -> Self {
        Self {
            client: Arc::new(Mutex::new(Client::new())),
            access_token: Arc::new(Mutex::new(String::new())),
            search_query: String::new(),
            search_results: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl epi::App for SpotifySearchApp {
    fn name(&self) -> &str {
        "Spotify Search App"
    }

    fn setup(
        &mut self,
        ctx: &egui::CtxRef,
        _frame: &mut epi::Frame,
        _storage: Option<&dyn epi::Storage>,
    ) {
        let client = self.client.clone();
        let access_token = self.access_token.clone();
        tokio::spawn(async move {
            let token = get_access_token(&*client.lock().await).await.unwrap();
            *access_token.lock().await = token;
        });

        let font_data = include_bytes!("jf-openhuninn-2.0.ttf");

        
        let mut fonts = egui::FontDefinitions::default();

        
        fonts.font_data.insert(
            "jf-openhuninn".to_owned(),
            std::borrow::Cow::Borrowed(font_data),
        );
        
        fonts
            .fonts_for_family
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "jf-openhuninn".to_owned());
        fonts
            .fonts_for_family
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "jf-openhuninn".to_owned());
        
        fonts.family_and_size.insert(
            egui::TextStyle::Body,
            (egui::FontFamily::Proportional, 14.0),
        ); //文字大小
        fonts.family_and_size.insert(
            egui::TextStyle::Button,
            (egui::FontFamily::Proportional, 14.0),
        ); // 按鈕大小 
        fonts.family_and_size.insert(
            egui::TextStyle::Heading,
            (egui::FontFamily::Proportional, 22.0),
        ); //標題大小

        // 套用
        ctx.set_fonts(fonts);
    }

    fn update(&mut self, ctx: &egui::CtxRef, frame: &mut epi::Frame) {
        let window_size = ctx.input().screen_rect.size(); // 擷取當前視窗大小

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Spotify Song Search");
            ui.horizontal(|ui| {
                ui.label("Search for a song:");
                ui.text_edit_singleline(&mut self.search_query);
            });
            if ui.button("Search").clicked() {
                let client = self.client.clone();
                let access_token = self.access_token.clone();
                let query = self.search_query.clone();
                let search_results = self.search_results.clone();
                tokio::spawn(async move {
                    let result = search_track(
                        &*client.lock().await,
                        &query,
                        &*access_token.lock().await,
                        1,
                        20,
                    )
                    .await;
                    let mut results = search_results.lock().await;
                    *results = match result {
                        Ok((tracks, _)) => tracks,
                        Err(_) => Vec::new(),
                    };
                });
            }

            // ?示?前窗口大小
            ui.label(format!("Window size: {:.0} x {:.0}", window_size.x, window_size.y));

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
fn main() {
    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let app = SpotifySearchApp::default();
        let mut native_options = eframe::NativeOptions::default();
        native_options.initial_window_size = Some(egui::vec2(458.0, 323.0)); //預設458x323
        eframe::run_native(Box::new(app), native_options);
    });
}
