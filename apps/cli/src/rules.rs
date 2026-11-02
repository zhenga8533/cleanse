use cleanse_engine::{Engine, Entry, Os, Rule, Target};

use crate::{format, write_json};

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Option<RulesCommand>,
    /// Print the rules as JSON, as rule files write them.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Debug, clap::Subcommand)]
enum RulesCommand {
    /// List every rule (the default)
    List,
    /// Show one rule in detail
    Show {
        /// The rule's id, like `javascript.node_modules`.
        id: String,
    },
}

pub fn run(engine: &Engine, args: &Args) -> crate::Result {
    match &args.command {
        None | Some(RulesCommand::List) if args.json => write_json(engine.rules().rules()),
        None | Some(RulesCommand::List) => {
            list(engine.rules().rules());
            Ok(())
        }
        Some(RulesCommand::Show { id }) => {
            let rule = engine
                .rules()
                .rules()
                .iter()
                .find(|rule| rule.id.0 == *id)
                .ok_or_else(|| format!("there's no rule `{id}`; `cleanse rules` lists them"))?;
            if args.json {
                return write_json(rule);
            }
            show(rule, engine.config().large_file_size);
            Ok(())
        }
    }
}

fn list(rules: &[Rule]) {
    let width = rules.iter().map(|rule| rule.id.0.len()).max().unwrap_or(0);
    for rule in rules {
        println!(
            "{:<width$}  {:<12} {:<16} {}",
            rule.id.0,
            rule.risk.to_string(),
            rule.category.to_string(),
            rule.name
        );
    }
}

fn show(rule: &Rule, large_file_size: u64) {
    println!("{} ({})", rule.name, rule.id);
    println!("  Category:    {}", rule.category);
    println!("  Tier:        {}", rule.risk);
    println!("  Why:         {}", rule.explain);
    if let Some(regenerate) = &rule.regenerate {
        println!("  Comes back:  {regenerate}");
    }
    if let Some(command) = &rule.advise {
        println!("  Clean with:  {command}");
    }
    match &rule.target {
        Target::ProjectArtifact {
            names,
            markers,
            contains,
            require_git_ignored,
        } => {
            println!("  Matches:     a folder named {}", names.join(", "));
            if !markers.is_empty() {
                println!("               next to any of {}", markers.join(", "));
            }
            if !contains.is_empty() {
                println!("               containing {}", contains.join(", "));
            }
            if *require_git_ignored {
                println!("               that Git ignores");
            }
        }
        Target::KnownLocation { locations, entry } => {
            let here: Vec<String> = locations
                .iter()
                .filter(|location| location.os == Os::current())
                .map(|location| format!("{:?}/{}", location.base, location.path))
                .collect();
            let what = match entry {
                Entry::Dir => "",
                Entry::File => "files at ",
                Entry::Any => "files and folders at ",
            };
            if here.is_empty() {
                println!("  Matches:     nothing on this operating system");
            } else {
                println!("  Matches:     {what}{}", here.join(", "));
            }
        }
        Target::LargeFile {} => {
            println!(
                "  Matches:     files taking {} or more on disk (large_file_size in your \
                 settings),",
                format::bytes(large_file_size)
            );
            println!("               outside the folders apps keep their data in");
        }
    }
    if let Some(owner) = &rule.owner {
        println!("  Kept while:  {} is running", owner.app);
    }
    if let Some(days) = rule.idle_days {
        println!("  Kept until:  unchanged for {days} days, and since the computer last started");
    }
    println!("  Tests:       {}", rule.tests.len());
}
