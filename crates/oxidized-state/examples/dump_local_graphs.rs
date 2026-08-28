use std::path::Path;
use surrealdb::engine::any;
use surrealdb::Value;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db_paths = vec![
        "/Users/steven/engineering/.aivcs/db",
        "/Users/steven/engineering/web-apps/.aivcs/db",
        "/Users/steven/engineering/code-governance/.aivcs/db",
        "/Users/steven/engineering/aivcs-repos/minimal-web-app/.aivcs/db",
    ];

    for db_path in db_paths {
        if !Path::new(db_path).exists() {
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
            for (idx, row) in rows.iter().enumerate().take(10) {
                println!("    [{}] {:?}", idx, row);
            }
        }
    }

    Ok(())
}
