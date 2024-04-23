use iced::{
    alignment::{self, Horizontal}, 
    executor,
    widget::{button, text_input, Button, Column, Container, Text, TextInput},
    Application, Command, Element, Settings,
};
/*use spotify_search_lib::spotify_search::{
    get_access_token, get_track_info, is_valid_spotify_url, print_album_info, print_track_infos,
    search_album_by_name, search_album_by_url, search_track,
}; */
use tokio;

struct SpotifySearchApp {
    search_input: String,
    search_input_state: iced::widget::text_input::State<()>, 
    search_button: iced::widget::button::State,
    search_result: String,
}

#[derive(Debug, Clone)]
enum Message {
    SearchInputChanged(String),
    SearchPressed,
}

impl iced::Application for SpotifySearchApp {
    type Executor = iced::executor::Default;
    type Message = Message;
    type Flags = ();
    type Theme = iced::theme::Theme;

    fn new(_flags: ()) -> (Self, Command<Self::Message>) {
        (
            Self {
                search_input: String::new(),
                search_input_state: text_input::State::new(),
                search_button: button::State::new(),
                search_result: String::from("PLEASE..."),
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        String::from("Spotify Search")
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        match message {
            Message::SearchInputChanged(input) => {
                self.search_input = input; 
            }
            Message::SearchPressed => {
                
                self.search_result = format!("搜索结果: {}", self.search_input);
            }
        }
        Command::none()
    }

    fn view(&self) -> Element<Self::Message> {
        let search_button = Button::new(Text::new("search"))
        .on_press(Message::SearchPressed)
        .padding(10);
        let content = Column::new()
            .align_items(alignment::Alignment::Center)
            .spacing(20)
            .push(Text::new(&self.search_result))
            .push(search_button)
            .push(Text::new(&self.search_result));

        Container::new(content)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center)
            .into()
    }
}

#[tokio::main]
async fn main() -> iced::Result {
    SpotifySearchApp::run(Settings::default())
}
