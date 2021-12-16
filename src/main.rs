use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::prelude::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str::FromStr;
use vega_lite_3::*;
use walkdir::WalkDir;

#[derive(Debug, Deserialize)]
struct Benchmarker {
    /// The Git repository to benchmark
    repository: String,
    /// A list of commands to run in preparation of the benchmark
    prepare: Vec<String>,
    /// The commands to be benchmarked with Hyperfine
    benchmarks: Vec<String>,
    /// The directory where the repository is cloned
    #[serde(default = "Benchmarker::tmp_dir")]
    repo_dir: String,
}

impl Benchmarker {
    fn from_file(path: &str) -> Result<Self> {
        // TODO: fetch from remote file if starts with HTTP
        let mut f = File::open(path)?;
        let mut s = String::new();
        f.read_to_string(&mut s)?;
        toml::from_str(&s).context("deserializing configuration")
    }

    fn tmp_dir() -> String {
        "/tmp/fineregr".to_owned()
        // tempdir::TempDir::new("fineregr")
        //     .expect("error creating temprary directory")
        //     .into_path()
        //     .to_str()
        //     .unwrap()
        //     .to_owned()
    }

    /// Clones the repository as a subdirectory of the current working directory
    fn clone_repo(&self) -> Result<()> {
        println!("Cloning {} to {}", self.repository, self.repo_dir);
        Command::new("git")
            .arg("clone")
            .arg(&self.repository)
            .arg(&self.repo_dir)
            .spawn()?
            .wait()?;

        Ok(())
    }

    fn run_prepare(&self) -> Result<()> {
        for cmd in &self.prepare {
            let args: Vec<&str> = cmd.split_whitespace().collect();
            let ret = Command::new(args[0])
                .args(&args[1..])
                .current_dir(&self.repo_dir)
                .spawn()?
                .wait()?;
            if !ret.success() {
                bail!("return code {:?}", ret);
            }
        }
        Ok(())
    }

    fn checkout(&self, sha: &str) -> Result<()> {
        Command::new("git")
            .arg("checkout")
            .arg(sha)
            .current_dir(&self.repo_dir)
            .spawn()?
            .wait()?;
        Ok(())
    }

    fn get_commits(&self) -> Result<Vec<String>> {
        let output = Command::new("git")
            .arg("rev-list")
            .arg("main")
            .current_dir(&self.repo_dir)
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        Ok(String::from_utf8(output.stdout)?
            .lines()
            .map(|l| l.to_owned())
            .collect())
    }

    fn commit_date(&self, sha: &str) -> Result<String> {
        let output = Command::new("git")
            .arg("log")
            .arg("--format=%ci")
            .arg("-n")
            .arg("1")
            .arg(sha)
            .current_dir(&self.repo_dir)
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        String::from_utf8(output.stdout).context("decoding git message")
    }

    fn commit_message(&self, sha: &str) -> Result<String> {
        let output = Command::new("git")
            .arg("log")
            .arg("--format=%B")
            .arg("-n")
            .arg("1")
            .arg(sha)
            .current_dir(&self.repo_dir)
            .stdout(Stdio::piped())
            .spawn()?
            .wait_with_output()?;
        String::from_utf8(output.stdout).context("decoding git message")
    }

    fn run(&self) -> Result<()> {
        // self.clone_repo()?;
        let out_dir = std::env::current_dir()?.join("results");
        if !out_dir.is_dir() {
            std::fs::create_dir_all(&out_dir)?;
        }
        for sha in self.get_commits()?.into_iter().take(20) {
            self.checkout(&sha)?;

            for bench in &self.benchmarks {
                let mut bench_sha = Sha256::new();
                bench_sha.update(bench);
                let bench_sha = format!("{:x}", bench_sha.finalize());
                let dir = out_dir.join(bench_sha);
                if !dir.is_dir() {
                    std::fs::create_dir(&dir)?;
                }
                let json_file = dir.join(format!("{}.json", sha));

                if !json_file.is_file() {
                    let success = match self.run_prepare() {
                        Ok(()) => {
                            let res = Command::new("hyperfine")
                                .arg("--export-json")
                                .arg(&json_file)
                                .arg("--warmup")
                                .arg("5")
                                .arg(bench)
                                .current_dir(&self.repo_dir)
                                .spawn()?
                                .wait()?;
                            res.success()
                        }
                        Err(e) => {
                            eprintln!("{:?}", e);
                            false
                        }
                    };
                    if !success {
                        let json_data = json!({
                            "results": [
                                {
                                    "command": bench,
                                    "git_sha": sha,
                                    "git_msg": self.commit_message(&sha)?,
                                    "git_date": self.commit_date(&sha)?,
                                }
                            ]
                        });
                        let mut f = File::create(json_file)?;
                        write!(f, "{}", json_data)?;
                    }
                }
            }
        }

        // Now process the results
        let mut plotdata: Vec<PlotData> = Vec::new();
        for json_path in WalkDir::new(&out_dir) {
            let json_path = json_path?.into_path();
            if json_path.is_file() && json_path.extension().map(|ext| ext.to_str().unwrap() == "json").unwrap_or(false) {
                let git_sha = json_path
                    .file_name()
                    .context("getting file name")?
                    .to_str()
                    .context("to str")?
                    .replace(".json", "");
                let git_msg = self.commit_message(&git_sha)?;
                let git_date = self.commit_date(&git_sha)?;
                let rf: ResultFile = serde_json::from_reader(File::open(&json_path)?)?;
                for res in rf.results {
                    let command = res.command;
                    if let Some(times) = res.times {
                        for time in times {
                            plotdata.push(PlotData {
                                git_sha: git_sha.clone(),
                                git_msg: git_msg.clone(),
                                git_date: git_date.clone(),
                                command: command.clone(),
                                time: Some(time),
                            })
                        }
                    } else {
                        plotdata.push(PlotData {
                            git_sha: git_sha.clone(),
                            git_msg: git_msg.clone(),
                            git_date: git_date.clone(),
                            command: command.clone(),
                            time: None,
                        })
                    }
                }
            }
        }

        self.plot(plotdata, &out_dir)?;

        Ok(())
    }

    fn plot(&self, plotdata: Vec<PlotData>, out_dir: &PathBuf) -> Result<()> {
        let vega_spec = json!(
            {
                "$schema": "https://vega.github.io/schema/vega-lite/v5.json",
                "description": "",
                "data": {"values": plotdata},
                "mark": {
                  "type": "point",
                  "extent": "min-max"
                },
                "config": {
                  "mark": {"invalid": null}
                },
                "encoding": {
                  "x": {"field": "git_date", "type": "nominal"},
                  "y": {
                    "field": "time",
                    "type": "quantitative",
                    "scale": {"zero": false}
                  },
                  "tooltip": [
                    {"field": "git_msg", "type": "nominal"},
                    {"field": "git_date", "type": "nominal"},
                    {"field": "git_sha", "type": "nominal"}
                  ],
                  "color": {
                    "condition": {
                      "test": "datum['time'] === null",
                      "value": "#f00"
                    }
                  },
                  "row": {
                    "field": "command"
                  }
                }
              }
        );
        let mut f = File::create(out_dir.join("index.html"))?;
        write!(f,include_str!("index.html"), vega_spec)?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct PlotData {
    git_sha: String,
    git_msg: String,
    git_date: String,
    command: String,
    time: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ResultFile {
    results: Vec<ResultEntry>,
}

#[derive(Debug, Deserialize)]
struct ResultEntry {
    command: String,
    times: Option<Vec<f64>>,
}

fn main() -> Result<()> {
    let cfg_path = std::env::args()
        .nth(1)
        .context("You should provide the path to the configuration file")?;
    let bench = Benchmarker::from_file(&cfg_path)?;

    bench.run()?;

    Ok(())
}