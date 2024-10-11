

pub struct OsuHelper {
    pub show: bool,
}

impl OsuHelper {
    pub fn new() -> Self {
        Self {
            show: false,
        }
    }



    pub fn render(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Osu! Helper");
            ui.label("建置中...");
        });
    }


    // 其他方法...
}

