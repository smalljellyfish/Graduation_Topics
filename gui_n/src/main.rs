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
use egui::{ColorImage, Context, TextureHandle};
use egui::{FontData, FontDefinitions, FontFamily};
use egui::ViewportBuilder;

use reqwest::Client;

use log::{error, info};
use simplelog::*;

use image::load_from_memory;
use std::default::Default;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use std::collections::HashMap;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::RwLock;
use tokio::task::JoinHandle;

use tokio::sync::mpsc::Sender;

// 定義 SpotifySearchApp結構，儲存程式狀態和數據
struct SearchApp {
    client: Arc<tokio::sync::Mutex<Client>>,
    access_token: Arc<tokio::sync::Mutex<String>>,
    search_query: String,
    search_results: Arc<tokio::sync::Mutex<Vec<Track>>>,
    osu_search_results: Arc<tokio::sync::Mutex<Vec<Beatmapset>>>,
    error_message: Arc<tokio::sync::Mutex<String>>,
    initialized: bool,
    is_searching: Arc<AtomicBool>,
    need_repaint: Arc<AtomicBool>,
    font_size: f32,
    show_relax_window: bool,
    relax_slider_value: i32,
    selected_beatmapset: Option<usize>,
    err_msg: Arc<tokio::sync::Mutex<String>>,
    receiver: Option<tokio::sync::mpsc::Receiver<(usize, Arc<TextureHandle>, (f32, f32))>>,
    cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>>,
    sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
}

//為上方實現Default trait，創建默認狀態
/*impl Default for SearchApp {
    fn default() -> Self {
        Self {
            client: Arc::new(AsyncMutex::new(Client::new())),
            access_token: Arc::new(AsyncMutex::new(String::new())),
            search_query: String::new(),
            search_results: Arc::new(AsyncMutex::new(Vec::new())),
            osu_search_results: Arc::new(AsyncMutex::new(Vec::new())),
            error_message: Arc::new(AsyncMutex::new(String::new())),
            initialized: false,
            is_searching: Arc::new(AtomicBool::new(false)),
            need_repaint: Arc::new(AtomicBool::new(false)),
            font_size: 14.0,
            show_relax_window: false,
            relax_slider_value: 0,
            selected_beatmapset: None,
            err_msg: Arc::new(AsyncMutex::new(String::new())),
            cover_textures: Arc::new(AsyncMutex::new(HashMap::new())),
            global_cover_textures: Arc::new(Mutex::new(HashMap::new())),
            osu_urls: Vec::new(),
        }
    }
}
*/

impl eframe::App for SearchApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let window_size = ctx.input(|i| i.screen_rect.size()); //當前視窗大小
        let base_window_size = egui::vec2(764.0,412.0); // 基準視窗大小
        let base_font_size = 14.0; // 基準字體大小
        let ctx_clone = ctx.clone();

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
            let osu_urls = vec![];
            let sender_clone = self.sender.clone();
            tokio::spawn(async move {
                load_all_covers(osu_urls.clone(), ctx_clone.into(), sender_clone).await;
            });

            let mut receiver = self.receiver.take().expect("Receiver already taken");
            let cover_textures = self.cover_textures.clone();
            let need_repaint_clone = self.need_repaint.clone();

            tokio::spawn(async move {
                while let Some((id, texture, dimensions)) = receiver.recv().await {
                    let mut textures = cover_textures.write().await;
                    textures.insert(id, Some((texture, dimensions)));
                    need_repaint_clone.store(true, Ordering::SeqCst);
                }
            });
    
            self.initialized = true;

            let access_token = self.access_token.clone();
            let error_message = self.error_message.clone();
            let client_clone = client.clone();
            tokio::spawn(async move {
                let client_guard = client_clone.lock().await;
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

            let err_msg = {
                let err_msg_lock = self.err_msg.clone();
                let err_msg = tokio::spawn(async move { err_msg_lock.lock().await.clone() });
                err_msg
            };

            let ctx_clone = ctx.clone();
            let err_msg_clone = self.err_msg.clone();
            tokio::spawn(async move {
                let err_msg = err_msg_clone.lock().await;
                if !err_msg.is_empty() {
                    ctx_clone.request_repaint();
                    egui::Window::new("Error").show(&ctx_clone, |ui| {
                        ui.label(&err_msg.to_string());
                    });
                }
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| {

            
            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
            ui.spacing_mut().window_margin = egui::Margin::symmetric(0.0, 0.0);
            let window_size = ctx.input(|i| i.screen_rect.size()); // 獲取當前視窗大小
            ui.label(format!("Window size: {} x {}", window_size.x, window_size.y));

            // 緊接著顯示 "Search for a song:" 標籤，無額外間距
            ui.heading("Search for a song:");
            ui.add_space(5.0); // 控制標籤和搜尋框之間的間距
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
                    self.perform_search(ctx.clone());
                }
            });
            let text_style = egui::TextStyle::Body.resolve(ui.style());
            let mut new_text_style = text_style.clone();
            new_text_style.size = self.font_size;
            ui.style_mut()
                .text_styles
                .insert(egui::TextStyle::Body, new_text_style);
            
            if let Ok(err_msg_guard) = self.err_msg.try_lock() {
                ui.label(format!("{}", *err_msg_guard));
            }
        
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
                                        ui.label("------------------------------------");
                                        ui.add_space(10.0);
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
                                        let mut sorted_beatmaps = selected_beatmapset.beatmaps.clone();
                                        sorted_beatmaps.sort_by(|easier, harder| harder.difficulty_rating.partial_cmp(&easier.difficulty_rating).unwrap());
                                        for beatmap in &sorted_beatmaps {
                                            let beatmap_info = format!(
                                                "Difficulty: {}\nMode: {}\nStatus: {}\nLength: {} min {}s \nVersion: {}",
                                                beatmap.difficulty_rating,
                                                beatmap.mode,
                                                beatmap.status,
                                                beatmap.total_length/60,
                                                beatmap.total_length%60,
                                                beatmap.version
                                            );
                                            ui.label(beatmap_info);
                                            ui.add_space(10.0);
                                            ui.label("------------------------------------");
                                            ui.add_space(10.0);
                                        }
                                        if ui.button("back").clicked(){
                                            self.selected_beatmapset = None;
                                        };
                                    } else {
                                        for (index, beatmapset) in osu_search_results_guard.iter().enumerate() {
                                            ui.horizontal(|ui| {
                                                // 嘗試非阻塞地獲取鎖
                                                if let Ok(textures) = self.cover_textures.try_read() {
                                                    // 處理兩層 Option
                                                    if let Some(Some((texture, size))) = textures.get(&index) {
                                                        let max_size = 100.0; // 設置最大尺寸
                                                        let aspect_ratio = size.0 as f32 / size.1 as f32; // 計算圖片的寬高比
                                                        let mut image_size = egui::Vec2::new(max_size, max_size / aspect_ratio); // 根據寬高比計算適當的尺寸
                                                        
                                                        // 確保圖片不超出最大尺寸
                                                        if image_size.x > max_size {
                                                            image_size.x = max_size;
                                                            image_size.y = max_size / aspect_ratio;
                                                        }
                                                        if image_size.y > max_size {
                                                            image_size.y = max_size;
                                                            image_size.x = max_size * aspect_ratio;
                                                        }
                                                        
                                                        let image_source = (texture.id(), image_size);
                                                        ui.image(image_source);
                                                        info!("Displaying image for index: {}", index);
                                                    } else {
                                                        ui.label("Loading cover...");
                                                    }
                                                } else {
                                                    ui.label("Waiting for lock...");
                                                }
                                                ui.add_space(10.0);
                                            
                                                ui.vertical(|ui| {
                                                    if ui.button(format!("{} - {} (by {})", beatmapset.title, beatmapset.artist, beatmapset.creator)).clicked() {
                                                        self.selected_beatmapset = Some(index);
                                                    }
                                                });
                                            });                                            
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
async fn load_all_covers(
    urls: Vec<String>,
    ctx: egui::Context,
    sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
) {
    let client = Client::new();
    for (index, url) in urls.into_iter().enumerate() {
        info!("Loading cover from URL: {}", url);
        match client.get(&url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    match response.bytes().await {
                        Ok(bytes) => match load_from_memory(&bytes) {
                            Ok(image) => {
                                info!("Successfully loaded image from memory for URL: {}", url);
                                let color_image = ColorImage::from_rgba_unmultiplied(
                                    [image.width() as usize, image.height() as usize],
                                    &image.to_rgba8(),
                                );
                                let texture = ctx.load_texture(
                                    format!("cover_{}", index),
                                    color_image,
                                    Default::default(),
                                );
                                let texture = Arc::new(texture);
                                let size = (image.width() as f32, image.height() as f32);
                                match sender.send((index, texture, size)).await {
                                    Ok(_) => info!("Successfully sent texture for URL: {}", url),
                                    Err(e) => error!(
                                        "Failed to send texture for URL: {}, error: {:?}",
                                        url, e
                                    ),
                                }
                            }
                            Err(e) => error!(
                                "Failed to load image from memory for URL: {}, error: {:?}",
                                url, e
                            ),
                        },
                        Err(e) => error!(
                            "Failed to get bytes from response for URL: {}, error: {:?}",
                            url, e
                        ),
                    }
                } else {
                    error!(
                        "Failed to load cover for URL: {}, status code: {}",
                        url,
                        response.status()
                    );
                }
            }
            Err(e) => error!("Failed to send request for URL: {}, error: {:?}", url, e),
        }
    }
}


impl SearchApp {
    fn new(
        client: Arc<tokio::sync::Mutex<Client>>,
        sender: Sender<(usize, Arc<TextureHandle>, (f32, f32))>,
        receiver: tokio::sync::mpsc::Receiver<(usize, Arc<TextureHandle>, (f32, f32))>,
        cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>>,
        need_repaint: Arc<AtomicBool>,
    ) -> Self {
        Self {
            client,
            access_token: Arc::new(tokio::sync::Mutex::new(String::new())),
            search_query: String::new(),
            search_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            osu_search_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            error_message: Arc::new(tokio::sync::Mutex::new(String::new())),
            initialized: false,
            is_searching: Arc::new(AtomicBool::new(false)),
            need_repaint,
            font_size: 14.0,
            show_relax_window: false,
            relax_slider_value: 0,
            selected_beatmapset: None,
            err_msg: Arc::new(tokio::sync::Mutex::new(String::new())),
            cover_textures,
            sender,
            receiver: Some(receiver),
        }
    }

    fn perform_search(&mut self, ctx: egui::Context) -> JoinHandle<Result<()>> {
        let client = self.client.clone();
        let query = self.search_query.clone();
        let search_results = self.search_results.clone();
        let osu_search_results = self.osu_search_results.clone();
        let is_searching = Arc::clone(&self.is_searching);
        let need_repaint = self.need_repaint.clone();
        let error_message = self.error_message.clone();
        let err_msg = self.err_msg.clone();
        let sender = self.sender.clone();

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
            let spotify_result: Result<Vec<Track>, _> = if query.starts_with("http://")
                || query.starts_with("https://")
            {
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
                        *err_msg.lock().await = error_msg.to_string();
                        log::error!("{}", error_msg);
                        Err(anyhow::anyhow!(error_msg))
                    }
                } else {
                    let error_msg = "你疑似輸入URL,但它是不正確的。";
                    *error = error_msg.to_string();
                    *err_msg.lock().await = error_msg.to_string();
                    log::error!("{}", error_msg);
                    Err(anyhow::anyhow!(error_msg))
                }
            } else {
                // 假設 limit 和 offset 是您需要提供的參數
                let limit = 10;
                let offset = 0;
                search_track(&*client.lock().await, &query, &spotify_token, limit, offset)
                    .await
                    .map(|(tracks, _)| tracks)
            };

            match spotify_result {
                Ok(tracks) => {
                    let mut search_results = search_results.lock().await;
                    *search_results = tracks;
                }
                Err(e) => {
                    let error_msg = format!("Error searching Spotify: {:?}", e);
                    *error = error_msg.clone();
                    log::error!("{}", error_msg);
                }
            }

            // Osu search
            match get_beatmapsets(&*client.lock().await, &osu_token, &query).await {
                Ok(results) => {
                    info!("osu_search_results: {:?}", results);
                    let mut osu_urls = Vec::new();
                    for beatmapset in &results {
                        if let Some(cover_url) = &beatmapset.covers.cover {
                            osu_urls.push(cover_url.clone());
                        }
                    }
                    *osu_search_results.lock().await = results;
                    let ctx_clone = ctx.clone();
                    let sender_clone = sender.clone();

                    tokio::spawn(async move {
                        load_all_covers(osu_urls, ctx_clone, sender_clone).await;
                    });
                }
                Err(e) => {
                    error!("Error searching Osu: {}", e);
                    *error = format!("Error searching Osu: {:?}", e);
                }
            }

            is_searching.store(false, Ordering::SeqCst);
            need_repaint.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}
#[tokio::main]
async fn main() {
    // 初始化日誌
    let log_file = std::fs::File::create("output.log").unwrap();
    let mut config_builder = simplelog::ConfigBuilder::new();
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

    // 初始化 HTTP 客戶端
    let client = Arc::new(tokio::sync::Mutex::new(Client::new()));
    let (sender, receiver) = tokio::sync::mpsc::channel(100);

    // 定義 cover_textures
    let cover_textures: Arc<RwLock<HashMap<usize, Option<(Arc<TextureHandle>, (f32, f32))>>>> = Arc::new(RwLock::new(HashMap::new()));
    let need_repaint = Arc::new(AtomicBool::new(false));

    // 創建 SearchApp 實例
    let app = SearchApp::new(client.clone(), sender, receiver, cover_textures.clone(), need_repaint.clone());

    // 設定初始視窗大小
    let mut native_options = eframe::NativeOptions::default();
    native_options.viewport = ViewportBuilder {
        title: Some(String::from("Search App")),
    inner_size: Some(egui::Vec2::new(600.0, 300.0)), 
    ..Default::default()
};


    // 運行應用
    if let Err(e) = eframe::run_native("Search App", native_options, Box::new(|cc| Box::new(app))) {
        eprintln!("Error running native app: {}", e);
    }
}

