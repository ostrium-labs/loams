//! `loams-live-worker`: one isolated Loams Live function worker (LV1 plan
//! Task 5), the same loop `loams live-worker` runs. The host starts it with
//! piped stdio and speaks `loams.live.worker.v1` frames to it.

fn main() -> std::process::ExitCode {
    loams_live_js::worker_main()
}
