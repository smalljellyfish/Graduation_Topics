# 關於我們的專題 - OSU小工具  
> 從osu找到spotify的同一首歌 以及 從spotify找到osu的同一首歌  
  
一切始於我們又又又換專題主題的那個晚上，本來unity要準備開始了，但組員的一番提議，我們從C#鬼轉到Rust的程式開發  
耗時6個月製作，做出來的東西我還蠻滿意的   ~~至少我覺得專題展覽可以過拉~~  
  
---
  
## 如何製作的?
> 以什麼語言製作的？為什麼?以及製作過程依參數格式化輸出
  
我們以Rust製作  
**因為Rust有媲美C++的效率，又比C++安全很多***(我恨你空指標)*  
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
## 詳細的程式
> 裡面有什麼東西  
  
這是我們應用程式最基礎介面的程式
```rust
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
```
