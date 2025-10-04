use subtitle_fast::{Configuration, YPlaneStreamProvider};
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
async fn mock_backend_produces_stream() {
    let mut config = Configuration::default();
    config.backend = subtitle_fast::Backend::Mock;
    let provider = config.create_provider().expect("mock backend available");
    let mut stream = provider.into_stream();
    let mut frames = Vec::new();
    while let Some(frame) = stream.next().await {
        frames.push(frame.unwrap());
        if frames.len() == 3 {
            break;
        }
    }
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].width(), 640);
}
