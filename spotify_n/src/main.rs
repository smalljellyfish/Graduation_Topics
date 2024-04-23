use spotify_search_lib::spotify_search::{
    get_access_token, is_valid_spotify_url, get_track_info, print_track_infos, search_track, search_album_by_url, search_album_by_name,
 print_album_info};
use tokio;


#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let access_token = get_access_token(&client).await?;

    println!("Enter song name or Spotify URL: ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    let input = input.trim();

    if input.starts_with("http://")
        || input.starts_with("https://")
        || input.starts_with("open.spotify.com")
    {
        if !is_valid_spotify_url(input) {
            println!("你疑似輸入了 URL，但它不正確。");
            return Ok(());
        }
        fn extract_track_id_from_url(url: &str) -> Option<&str> {
            url.rsplit('/').next()
        }
        if input.contains("open.spotify.com/track/") {
            let track_id = extract_track_id_from_url(input).unwrap();
            let track_info = get_track_info(&client, track_id, &access_token).await?;
            print_track_infos(vec![track_info]);
            
        } else if input.contains("open.spotify.com/album/") {
            // 如果输入包含album URL，直接使用这个URL进行专辑搜索

            // 去除换行
            let album_url = input.trim();

            match search_album_by_url(&client, album_url, &access_token).await {
                Ok(album) => {
                print_album_info(&album);
                }
                Err(e) => println!("ERROR: {}", e),
            }
        }
         else {
            println!("你疑似輸入了 URL，但它不正確。");
        }
    } else {
        println!("請選擇搜尋類型：");
        println!("1. 歌曲");
        println!("2. 專輯");
        let mut choice = String::new();
        std::io::stdin().read_line(&mut choice).unwrap();
        let choice: &str = choice.trim();
        match choice {
            "1" => {
                let limit = 10;
                let mut page = 1;
                let (track_infos, total_pages) =
                    search_track(&client, input, &access_token, page, limit).await?;
                print_track_infos(track_infos);
                println!(
                    "目前在第{}頁，總共{}頁。请输入您要的頁數，或者輸入'exit'退出：",
                    page, total_pages
                );

                loop {
                    let mut action = String::new();
                    std::io::stdin().read_line(&mut action).unwrap();
                    if action.trim().eq("exit") {
                        break;
                    } else if let Ok(requested_page) = action.trim().parse::<u32>() {
                        if requested_page > 0 && requested_page <= total_pages {
                            page = requested_page;
                            let (track_infos, _) =
                                search_track(&client, input, &access_token, page, limit).await?;
                            print_track_infos(track_infos);
                            println!(
                                "目前在第{}頁，共计{}頁。请输入您要的頁數，或者輸入'exit'退出：",
                                page, total_pages
                            );
                        } else {
                            println!(
                                "輸入的頁數錯誤，請輸入1到{}的數字，或者輸入'exit'退出：",
                                total_pages
                            );
                        }
                    } else {
                        println!("錯誤，請輸入頁數數字，或輸入exit給我滾");
                    }
                }
            }

            "2" => {
                let album_name = input;
                let limit = 20; // 限制每頁顯示最多26張專輯
                let page = 1;

                
                let (albums, _) =
                    search_album_by_name(&client, album_name, &access_token, page, limit).await?;

                
                albums.iter().enumerate().for_each(|(index, album)| {
                    println!(
                        "{}. {} - {} [{}]",
                        char::from(b'a' + index as u8), // 将索引转换为字母
                        album.name,
                        album
                            .artists
                            .iter()
                            .map(|a| a.name.as_str())
                            .collect::<Vec<&str>>()
                            .join(", "),
                        album
                            .external_urls
                            .get("spotify")
                            .unwrap_or(&String::from("无URL"))
                    );
                });

                println!("請選擇專輯（a, b, c, ...）或輸入'exit'退出：");
                let mut choice = String::new();
                std::io::stdin().read_line(&mut choice).unwrap();
                let choice = choice.trim().to_lowercase();

                if choice == "exit" {
                    return Ok(());
                } else {
                    let index = choice.chars().next().unwrap() as usize - 'a' as usize;
                    if index < albums.len() {
                        let selected_album = &albums[index];
                        print_album_info(selected_album); 
                    } else {
                        println!("無效的選擇");
                    }
                }
            }
            _ => {
                println!("無效，請輸入1或2");
            }
        }
    }
    Ok(())
}
