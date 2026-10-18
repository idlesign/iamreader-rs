fn main() -> Result<(), Box<dyn std::error::Error>> {
    let build_date = if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
        epoch
            .parse::<i64>()
            .ok()
            .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
            .map(|dt| dt.format("%y%m%d").to_string())
            .unwrap_or_else(now_yyyymmdd)
    } else {
        now_yyyymmdd()
    };
    println!("cargo:rustc-env=BUILD_DATE={}", build_date);
    #[cfg(feature = "slint-ui")]
    slint_build::compile("ui/main.slint")?;

    Ok(())
}

fn now_yyyymmdd() -> String {
    chrono::Utc::now().format("%y%m%d").to_string()
}
