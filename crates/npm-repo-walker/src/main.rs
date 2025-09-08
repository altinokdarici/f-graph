use anyhow::Result;
use npm_repo_walker::PackageWalker;
use std::env;
use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    let package_path = if args.len() > 1 {
        PathBuf::from(&args[1])
    } else {
        // Use current directory by default
        env::current_dir()?
    };

    if !package_path.exists() {
        eprintln!("Error: Path {:?} does not exist", package_path);
        std::process::exit(1);
    }

    let package_json_path = package_path.join("package.json");
    if !package_json_path.exists() {
        eprintln!("Error: No package.json found at {:?}", package_json_path);
        std::process::exit(1);
    }

    println!("Starting npm repo walker at: {:?}", package_path);
    println!("==========================================");

    let walker = PackageWalker::new(package_path);

    let start_time = Instant::now();
    match walker.discover_packages().await {
        Ok(discovered) => {
            let duration = start_time.elapsed();
            println!(
                "Discovered {} packages in {:.3}s",
                discovered.len(),
                duration.as_secs_f64()
            );
            println!("\nDiscovery completed successfully!");
            println!("==========================================");

            // for (name, package) in discovered.iter() {
            //     // println!("\n📦 Package: {}", name);
            //     if let Some(version) = &package.version {
            //         // println!("   Version: {}", version);
            //     }
            //     println!("   Path: {:?}", package.path);
            //     if !package.dependencies.is_empty() {
            //         println!("   Dependencies: {}", package.dependencies.join(", "));
            //     } else {
            //         println!("   Dependencies: None");
            //     }
            // }
        }
        Err(e) => {
            eprintln!("Error during discovery: {}", e);

            // Print the full error chain for debugging
            let mut source = e.source();
            while let Some(err) = source {
                eprintln!("  Caused by: {}", err);
                source = err.source();
            }

            std::process::exit(1);
        }
    }

    Ok(())
}
