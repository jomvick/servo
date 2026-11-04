use std::process::ExitCode;

fn main() -> ExitCode {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_owned());
    if url::Url::parse(&url).is_err() {
        eprintln!("invalid URL: {url}");
        return ExitCode::FAILURE;
    }
    match webview_ui::run(url) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        },
    }
}
