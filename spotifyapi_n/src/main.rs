use reqwest::blocking::Client;
use serde::Deserialize;

#[derive(Deserialize)]
struct AuthResponse {
    access_token: String,
}

fn main() {
    // Set the Spotify client ID and secret directly in the code
    let client_id = "1fd222ecc408444492cbfb09286dd22d";
    let client_secret = "6dbf60888d8e48fe8c677bca829b525d";

    // Create a reqwest client
    let client = Client::new();

    // Build the request URL
    let auth_url = "https://accounts.spotify.com/api/token";
    let body = "grant_type=client_credentials";
    let auth_header = base64::encode(format!("{}:{}", client_id, client_secret));
    let request = client
        .post(auth_url)
        .header("Authorization", format!("Basic {}", auth_header))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body);

    // Send the request and handle the response
    let response = request.send().expect("Failed to send request");
    let auth_response: AuthResponse = response.json().expect("Failed to parse response");

    // Print the access token
    println!("Access token: {}", auth_response.access_token);
}