use std::env;
use std::path::Path;
use surrealdb::engine::any;
use surrealdb::Value;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Paths come from argv or AIVCS_DUMP_DB_PATHS (colon/semicolon-separated).
    // No maintainer home paths are baked into the tree.
    let mut db_paths: Vec<String> = env::args().skip(1).collect();
    if db_paths.is_empty() {
        if let Ok(raw) = env::var("AIVCS_DUMP_DB_PATHS") {
            db_paths = raw
                .split(|c| c == ':' || c == ';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
    }
    if db_paths.is_empty() {
        eprintln!(
            "usage: dump_local_graphs <db-path>...\n             or set AIVCS_DUMP_DB_PATHS to a colon/semicolon-separated list"
        );
        std::process::exit(2);
    }

    for db_path in db_paths {
        if !Path::new(&db_path).exists() {
            continue;
        }
        println!("\n==================================================");
        println!("  READING LOCAL AIVCS GRAPH: {}", db_path);
        println!("==================================================");

        let url = format!("surrealkv://{}", db_path);
        let db = any::connect(&url).await?;
        db.use_ns("aivcs").use_db("main").await?;

        let tables = vec![
            "commits",
            "snapshots",
            "graph_edges",
            "runs",
            "run_events",
            "releases",
            "decisions",
            "memories",
        ];
        for table in tables {
            let query = format!("SELECT * FROM {};", table);
            let mut res = db.query(query).await?;
            let rows: Vec<Value> = res.take(0)?;
            println!("  Table '{}': {} records found", table, rows.len());
            for row in rows.iter().take(5) {
                println!("    {:?}", row);
            }
            if rows.len() > 5 {
                println!("    ... and {} more", rows.len() - 5);
            }
        }
    }
    Ok(())
}
