use std::ffi::OsStr;

fn main() -> miette::Result<()> {
    if std::env::var_os("VIZ_UPDATE_CHECK").as_deref() == Some(OsStr::new("1")) {
        viz::update_check::run_background_check();
        return Ok(());
    }

    let args: Vec<String> = std::env::args().collect();
    viz::update_check::check_for_update(viz::upgrade::VERSION, &args);
    viz::cli::run()
}
