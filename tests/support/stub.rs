use serde_json::{Value, json};
use std::{
    env, fs,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
};
fn main() {
    let role = env::current_exe()
        .unwrap()
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_lowercase();
    let args: Vec<String> = env::args().skip(1).collect();
    // 保守 worker（Claude Code CLI）のスタブ。
    if role == "claude" {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        // デスクトップセッション由来の環境変数が worker へ漏れていないかをテストで確認するために記録する。
        let inherited: Vec<&str> = ["CLAUDECODE", "CLAUDE_CODE_SIMPLE", "ANTHROPIC_BASE_URL"]
            .into_iter()
            .filter(|k| env::var_os(k).is_some())
            .collect();
        let oauth_token_kept = env::var_os("CLAUDE_CODE_OAUTH_TOKEN").is_some();
        fs::write(env::var_os("HOOK_STUB_PROMPT").unwrap(),json!({"args":args,"prompt":input,"worker_env":env::var("SCRIPTURA_WORKER").ok(),"inherited_host_env":inherited,"oauth_token_kept":oauth_token_kept}).to_string()).unwrap();
        if let Ok(p) = env::var("HOOK_STUB_WRITES") {
            let writes: Value = serde_json::from_str(&p).unwrap();
            for (path, value) in writes.as_object().unwrap() {
                let target = PathBuf::from(path);
                if let Some(p) = target.parent() {
                    fs::create_dir_all(p).unwrap();
                }
                fs::write(target, value.as_str().unwrap()).unwrap();
            }
        }
        if let Ok(remote) = env::var("HOOK_STUB_GITHUB_AFTER_WORKER") {
            Command::new(env::var_os("HOOK_REAL_GIT").unwrap())
                .args(["remote", "set-url", "--push", "origin", &remote])
                .status()
                .unwrap();
        }
        if env::var("HOOK_STUB_HEAD_CHANGE").is_ok() {
            let git = env::var_os("HOOK_REAL_GIT").unwrap();
            fs::write("source.txt", "concurrent commit").unwrap();
            assert!(
                Command::new(&git)
                    .args(["add", "source.txt"])
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(
                Command::new(git)
                    .args(["commit", "-m", "concurrent"])
                    .stdout(Stdio::null())
                    .status()
                    .unwrap()
                    .success()
            );
        }
        std::process::exit(
            env::var("HOOK_STUB_EXIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        );
    } else if role == "gh" {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        if let Ok(path) = env::var("HOOK_STUB_GH_LOG") {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .unwrap();
            writeln!(file, "{input}").unwrap();
        }
        let default=json!({"data":{"repository":{"ref":{"associatedPullRequests":{"nodes":[],"pageInfo":{"hasNextPage":false,"endCursor":null}}}}}}).to_string();
        let mut response = env::var("HOOK_STUB_GH_JSON_FILE")
            .ok()
            .map(|p| fs::read_to_string(p).unwrap())
            .or_else(|| env::var("HOOK_STUB_GH_JSON").ok())
            .unwrap_or(default);
        if let Ok(threshold) = env::var("HOOK_STUB_GH_BLOCK_AFTER") {
            let counter =
                PathBuf::from(env::var_os("HOOK_STUB_PROMPT").unwrap()).with_extension("gh-count");
            let n = fs::read_to_string(&counter)
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0)
                + 1;
            fs::write(counter, n.to_string()).unwrap();
            if n > threshold.parse::<usize>().unwrap() {
                response=json!({"data":{"repository":{"ref":{"associatedPullRequests":{"nodes":[{"isCrossRepository":true,"headRefName":"main","headRepository":{"nameWithOwner":"owner/repo"},"baseRepository":{"nameWithOwner":"other/repo"}}],"pageInfo":{"hasNextPage":false}}}}}}).to_string();
            }
        }
        println!("{response}");
        std::process::exit(
            env::var("HOOK_STUB_GH_EXIT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
        );
    } else if role == "pipe-parent" || role == "detached-parent" || role == "null-parent" {
        #[cfg(windows)]
        if role == "detached-parent" {
            unsafe {
                #[link(name = "kernel32")]
                unsafe extern "system" {
                    fn GetStdHandle(which: u32) -> *mut core::ffi::c_void;
                    fn SetHandleInformation(
                        handle: *mut core::ffi::c_void,
                        mask: u32,
                        flags: u32,
                    ) -> i32;
                }
                for id in [-10i32, -11, -12] {
                    let handle = GetStdHandle(id as u32);
                    if !handle.is_null() && handle as isize != -1 {
                        assert_ne!(SetHandleInformation(handle, 1, 0), 0);
                    }
                }
            }
        }
        let exe = env::current_exe()
            .unwrap()
            .with_file_name("pipe-grandchild.exe");
        let child = Command::new(exe)
            .stdin(if role != "pipe-parent" {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .stdout(if role != "pipe-parent" {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .stderr(if role != "pipe-parent" {
                Stdio::null()
            } else {
                Stdio::inherit()
            })
            .spawn()
            .unwrap();
        fs::write(
            env::var_os("HOOK_CHILD_PID").unwrap(),
            child.id().to_string(),
        )
        .unwrap();
        if env::var("HOOK_PARENT_SLEEP").is_ok() {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    } else if role == "pipe-grandchild" {
        eprintln!("diagnostic inherited");
        if let Some(path) = env::var_os("HOOK_CHILD_EXIT") {
            for _ in 0..600 {
                if PathBuf::from(&path).exists() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        } else {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    } else if role == "managed-host" {
        let exe = env::current_exe()
            .unwrap()
            .with_file_name("pipe-parent.exe");
        let c = scriptura::process::command(exe.to_str().unwrap());
        let _ = scriptura::process::capture(c, None, std::time::Duration::from_secs(60), true);
    } else if role == "git" {
        if args.iter().any(|a| a == "push") && env::var("HOOK_GIT_FAIL_PUSH").is_ok()
            || args.iter().any(|a| a == "commit") && env::var("HOOK_GIT_FAIL_COMMIT").is_ok()
        {
            std::process::exit(1)
        }
        let status = Command::new(env::var_os("HOOK_REAL_GIT").unwrap())
            .args(&args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .unwrap();
        if status.success()
            && env::var("HOOK_GIT_UNCERTAIN_OPERATION")
                .is_ok_and(|operation| args.iter().any(|arg| arg == &operation))
        {
            let child = Command::new(
                env::current_exe()
                    .unwrap()
                    .with_file_name("pipe-grandchild.exe"),
            )
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
            fs::write(
                env::var_os("HOOK_CHILD_PID").unwrap(),
                child.id().to_string(),
            )
            .unwrap();
        }
        std::process::exit(status.code().unwrap_or(1));
    } else if role == "filter" {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input).unwrap();
        print!("{}", "x".repeat(30000));
    } else {
        eprintln!("unsupported fixture role {role}");
        std::process::exit(2)
    }
}
