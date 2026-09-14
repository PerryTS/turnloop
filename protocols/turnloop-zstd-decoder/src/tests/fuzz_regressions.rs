#[test]
fn test_all_artifacts() {
    extern crate std;
    use crate::decoding::BlockDecodingStrategy;
    use crate::decoding::FrameDecoder;
    use std::fs;
    use std::fs::File;

    let mut frame_dec = FrameDecoder::new();

    let mut executed = 0;
    for file in fs::read_dir("./fuzz/artifacts/decode").unwrap() {
        let file_name = file.unwrap().path();

        let fnstr = file_name.file_name().unwrap().to_str().unwrap();
        if !fnstr.starts_with("crash-") {
            continue;
        }

        executed += 1;
        let mut f = File::open(file_name.clone()).unwrap();

        /* ignore errors. It just should never panic on invalid input */
        let _: Result<_, _> = frame_dec
            .reset(&mut f)
            .and_then(|()| frame_dec.decode_blocks(&mut f, BlockDecodingStrategy::All));
    }
    assert!(executed > 0, "fuzz regressions must execute");
}
