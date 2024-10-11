# 關於我們的專題 - OSU小工具  
> 從osu找到spotify的同一首歌 以及 從spotify找到osu的同一首歌  
  
一切始於我們又又又換專題主題的那個晚上，本來unity要準備開始了，但組員的一番提議，我們從C#鬼轉到Rust的程式開發  
耗時6個月製作，做出來的東西我還蠻滿意的   ~~至少我覺得專題展覽可以過拉~~  
  
---
  
## 如何製作的?
> 以什麼語言製作的？為什麼?以及製作過程
  
我們以Rust製作  
**因為Rust有媲美C++的效率，又比C++安全很多***(我恨你空指標)*  
在漫長的6個月中，我與我們的組員都十分配合，該寫程式的，該做報告的以及[每天打遊戲王的](https://github.com/Molaylay)  
至少 我們全都做完了。對吧  
   
---
  
## OSU小工具的流程圖
> 他是如何動的?

流程圖之後再放上來，還沒畫好 他還在打遊戲王

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
