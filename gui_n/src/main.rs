
/*use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_album_info, print_track_infos,
    search_album_by_name, search_album_by_url, search_track,
}; */
use tokio;
use spotify_search_lib::spotify_search::{search_track,get_access_token,Track,print_track_info_gui};
use tokio::runtime::Runtime;


use eframe::{egui, epi};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::Mutex;

struct SpotifySearchApp {
    client: Arc<Mutex<Client>>,
    access_token: Arc<Mutex<String>>,
    search_query: String,
    search_results: Arc<Mutex<Vec<Track>>>,  
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

    fn setup(&mut self, _ctx: &egui::CtxRef, _frame: &mut epi::Frame, _storage: Option<&dyn epi::Storage>) {
        let client = self.client.clone();
        let access_token = self.access_token.clone();
        tokio::spawn(async move {
            let token = get_access_token(&*client.lock().await).await.unwrap();
            *access_token.lock().await = token;
        });
    }


        fn update(&mut self, ctx: &egui::CtxRef, _frame: &mut epi::Frame) {
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
                        let result = search_track(&*client.lock().await, &query, &*access_token.lock().await, 1, 20).await;
                        let mut results = search_results.lock().await;
                        *results = match result {
                            Ok((tracks, _)) => tracks,
                            Err(_) => Vec::new(),
                        };
                    });
                }
                // 使用 ScrollArea ?示搜索?果
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Ok(search_results) = self.search_results.try_lock() {
                        if !search_results.is_empty() {
                            ui.label("Search Results:");
                            for track in search_results.iter() {
                                let formatted_result = print_track_info_gui(track);                  //這裡調用函數
                                ui.label(&formatted_result);
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
            let native_options = eframe::NativeOptions::default();
            eframe::run_native(Box::new(app), native_options);
        });
    }

