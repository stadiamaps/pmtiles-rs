//! A mock HTTP server for the backend tests.

use mockito::{Matcher, Mock, ServerGuard};

/// Serves `file` for every `GET` request, answering `Range: bytes={start}-{end}` with
/// `206 Partial Content` and the requested bytes. Every response also gets `headers`.
pub async fn serve_file(server: &mut ServerGuard, file: &str, headers: &[(&str, &str)]) -> Mock {
    let data = std::fs::read(file).unwrap();
    let mut mock = server
        .mock("GET", Matcher::Any)
        .match_header("range", Matcher::Any)
        .with_status(206);
    for (name, value) in headers {
        mock = mock.with_header(*name, value);
    }
    mock.with_body_from_request(move |request| {
        let range = request.header("range")[0].to_str().unwrap();
        let (start, end) = range
            .strip_prefix("bytes=")
            .and_then(|range| range.split_once('-'))
            .unwrap();
        let start = start.parse::<usize>().unwrap().min(data.len());
        let end = end.parse::<usize>().unwrap().min(data.len() - 1);
        data.get(start..=end).unwrap_or_default().to_vec()
    })
    .create_async()
    .await
}
