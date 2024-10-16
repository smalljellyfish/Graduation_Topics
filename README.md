# 關於我們的專題 - OSU小工具  
> 從osu找到spotify的同一首歌 以及 從spotify找到osu的同一首歌  
  
一切始於我們又又又換專題主題的那個晚上，本來unity要準備開始了，但組員的一番提議，我們從C#鬼轉到Rust的程式開發  
耗時6個月製作，做出來的東西我還蠻滿意的   ~~至少我覺得專題展覽可以過拉~~  
  
---
  
## 如何製作的?
> 以什麼語言製作的？為什麼?以及製作過程依參數格式化輸出
  
我們以Rust製作  
**因為Rust有媲美C++的效率，又比C++安全很多** **(我恨你空指標)**  
在漫長的6個月中，我與我們的組員都十分配合，該寫程式的，該做報告的以及[每天打遊戲王的](https://github.com/Molaylay)  
至少 我們全都做完了。對吧  
   
---
  
## OSU小工具的流程圖
> 他是如何動的?
  
流程圖之後再放上來，還沒畫好 他還在打遊戲王  
  
```mermaid

```
  
## 開發時程(甘特圖)  
>我們在這期間都做了些什麼  
  
```mermaid
    gantt
        title 開發歷程
        dateFormat MM-DD
            axisFormat %y-%m-%d
            
            section 主題確認  
                組內討論:crit,active, discuss, 2024-03-02,2d
            section 可行性研究  
                Rust程式語言學習:active, done,after discuss,120d
            section APP製作  
                程式撰寫:active, done,after discuss,220d
                APP版面設計:active, done,after discuss,30d
                APP版面重設:active, done,2024-5-15,30d
            section 程式測試與修正  
                程式檢測:active, down,2024-5-1,100d
                程式檢測:active, down,2024-8-30,20d
                程式修正:active, down,2024-5-20,100d
                程式修正:active, down,2024-9-10,30d
            section 專題製作(報告,海報方面)  
                海報設計:crit, uxt,2024-9-30,35d
                簡報製作:crit, uxt,2024-9-30,35d

```
## 詳細的程式 - 簡單了解他是怎麼運作的
> 裡面是什麼動得很厲害  
  

就如上面流程圖(他目前還不存在)所展示的，我們是透過Spotify與OSU所提供的API來做最基本的運作

> API的連線(OSU) 
```rust
pub async fn get_osu_token(client: &Client, debug_mode: bool) -> Result<String, OsuError> {
    if debug_mode {
        debug!("開始獲取 Osu token");
    }

    let config = read_config(debug_mode).map_err(|e| {
        error!("讀取配置文件時出錯: {}", e);
        OsuError::ConfigError(format!("Error reading config: {}", e))
    })?;

    let client_id = &config.osu.client_id;
    let client_secret = &config.osu.client_secret;

    if debug_mode {
        debug!("成功讀取 Osu client_id 和 client_secret");
    }

    let url = "https://osu.ppy.sh/oauth/token";
    let params = [
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("grant_type", &"client_credentials".to_string()),
        ("scope", &"public".to_string()),
    ];

    if debug_mode {
        debug!("準備發送 Osu token 請求");
    }

    let response = client.post(url).form(&params).send().await.map_err(|e| {
        error!("發送 Osu token 請求時出錯: {}", e);
        OsuError::RequestError(e)
    })?;

    let token_response: TokenResponse = response.json().await.map_err(|e| {
        error!("解析 Osu token 回應時出錯: {}", e);
        OsuError::RequestError(e)
    })?;

    if debug_mode {
        debug!("成功獲取 Osu token");
    }

    Ok(token_response.access_token)
}
```
>輸入框的例外處理(連結錯誤)
```rust
pub fn is_valid_spotify_url(url: &str) -> Result<SpotifyUrlStatus, SpotifyError> {
    lazy_static! {
        static ref SPOTIFY_URL_REGEX: Regex = Regex::new(
            r"^https?://open\.spotify\.com/(track|album|playlist)/[a-zA-Z0-9]+(?:\?.*)?$"
        )
        .unwrap();
    }

    if let Ok(parsed_url) = url::Url::parse(url) {
        match parsed_url.domain() {
            Some("open.spotify.com") => {
                if SPOTIFY_URL_REGEX.is_match(url) {
                    Ok(SpotifyUrlStatus::Valid)
                } else {
                    Ok(SpotifyUrlStatus::Incomplete)
                }
            }
            Some(_) => {
                if url.contains("/track/") || url.contains("/album/") || url.contains("/playlist/")
                {
                    Ok(SpotifyUrlStatus::Invalid)
                } else {
                    Ok(SpotifyUrlStatus::NotSpotify)
                }
            }
            None => Ok(SpotifyUrlStatus::NotSpotify),
        }
    } else {
        Ok(SpotifyUrlStatus::NotSpotify)
    }
}
```
接下來確認輸入框的東西都沒問題的話，開始把從個別資料庫中的信息取出(如:歌名、作者名...等)，之後等待兩邊的資料回傳。
>回傳的結構體樣式(spotify)
```rust
pub struct Track {
    pub name: String,
    pub artists: Vec<Artist>,
    pub external_urls: HashMap<String, String>,
    pub album: Album,
    pub is_liked: Option<bool>,
    #[serde(skip)]
    pub index: usize,
}
```
>將資料放進結構體當中
```rust
pub async fn search_track(
    client: &Client,
    query: &str,
    token: &str,
    limit: u32,
    offset: u32,
    debug_mode: bool,
) -> Result<(Vec<TrackWithCover>, u32), SpotifyError> {
    let url = format!(
        "{}/search?q={}&type=track&limit={}&offset={}",
        SPOTIFY_API_BASE_URL, query, limit, offset
    );

    let response = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| SpotifyError::RequestError(e))?;

    if debug_mode {
        info!("Spotify API 請求詳情:");
        info!("  URL: {}", url);
        info!("收到回應狀態碼: {}", response.status());
    }

    let response_text = response
        .text()
        .await
        .map_err(|e| SpotifyError::RequestError(e))?;

    if debug_mode {
        info!("Spotify API 回應 JSON: {}", response_text);
    }

    let search_result: SearchResult =
        serde_json::from_str(&response_text).map_err(|e| SpotifyError::JsonError(e))?;

        match search_result.tracks {
            Some(tracks) => {
                let total_tracks = tracks.total;
                let total_pages = (total_tracks + limit - 1) / limit;

            if debug_mode {
                info!("找到 {} 首曲目，共 {} 頁", tracks.total, total_pages);
            }

            let track_infos: Vec<TrackWithCover> = tracks
                .items
                .into_iter()
                .enumerate()
                .map(|(index, track)| {
                    let cover_url = track.album.images.first().map(|img| img.url.clone());
                    let artists_names = track
                        .artists
                        .iter()
                        .map(|artist| artist.name.clone())
                        .collect::<Vec<String>>()
                        .join(", ");

                    if debug_mode {
                        if let Some(url) = &cover_url {
                            info!(
                                "處理曲目 {}: \"{}\" by {}",
                                index, track.name, artists_names
                            );
                            info!("  專輯封面 URL: {}", url);
                        } else {
                            error!(
                                "處理曲目 {} 時出錯: \"{}\" by {} - 缺少封面 URL",
                                index, track.name, artists_names
                            );
                        }
                    }

                    TrackWithCover {
                        name: track.name,
                        artists: track.artists,
                        external_urls: track.external_urls,
                        album_name: track.album.name,
                        cover_url,
                        index: index + (offset as usize),
                    }
                })
                .collect();

            if debug_mode {
                info!("成功處理 {} 首曲目", track_infos.len());
            }

            Ok((track_infos, total_pages))
        }
        None => Err(SpotifyError::ApiError("搜索結果中沒有找到曲目".to_string())),
    }
}
```