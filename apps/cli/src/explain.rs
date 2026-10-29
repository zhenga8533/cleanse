use std::io;
use std::path::PathBuf;

use cleanse_engine::{Engine, Explanation};
use cleanse_model::Risk;
use serde_json::json;

use crate::format;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The file or folder to explain.
    path: PathBuf,
    /// Print the explanation as JSON.
    #[arg(long)]
    json: bool,
}

pub fn run(engine: &Engine, args: &Args) -> crate::Result {
    let explanation = engine.explain(&args.path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("couldn't explain {}: {error}", args.path.display()),
        )
    })?;
    if args.json {
        let output = match &explanation {
            Explanation::Matched { candidate, .. } => {
                json!({ "matched": true, "candidate": candidate })
            }
            Explanation::Unmatched { protected } => {
                json!({ "matched": false, "protected": protected })
            }
        };
        serde_json::to_writer_pretty(io::stdout().lock(), &output)?;
        println!();
        return Ok(());
    }

    match explanation {
        Explanation::Unmatched { protected } => {
            println!("{}", args.path.display());
            println!(
                "  No rule recognizes this or a folder containing it, so Cleanse leaves it alone."
            );
            if let Some(what) = protected {
                println!("  It's also protected: {what}.");
            }
        }
        Explanation::Matched { candidate, rule } => {
            let found = &candidate.found;
            println!("{}", found.path.display());
            if found.path != std::path::absolute(&args.path)? {
                println!("  (the folder containing {})", args.path.display());
            }
            println!("  Rule:        {} ({})", rule.name, rule.id);
            let verdict = match candidate.risk() {
                Risk::Protected => "kept",
                Risk::Review => "needs your review",
                Risk::Safe | Risk::Regenerable => "can be removed",
            };
            println!("  Tier:        {}, {verdict}", candidate.risk());
            println!("  Why:         {}", rule.explain);
            if let Some(regenerate) = &rule.regenerate {
                println!("  Comes back:  {regenerate}");
            }
            println!(
                "  Size:        {} in {} files",
                format::size(&found.size),
                format::count(found.files)
            );
            let evidence: Vec<String> = found.evidence.iter().map(ToString::to_string).collect();
            if !evidence.is_empty() {
                println!("  Evidence:    {}", evidence.join("; "));
            }
            if candidate.concerns().is_empty() {
                println!("  Concerns:    none");
            } else {
                println!("  Concerns:");
                for concern in candidate.concerns() {
                    let kind = if concern.blocks() {
                        "keeps it"
                    } else {
                        "warning"
                    };
                    println!("    - {concern} ({kind})");
                }
            }
        }
    }
    Ok(())
}
