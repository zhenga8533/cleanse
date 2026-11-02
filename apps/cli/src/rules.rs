use cleanse_engine::{Engine, Entry, Os, Rule, Target};

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Option<RulesCommand>,
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
            show(rule);
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

fn show(rule: &Rule) {
    println!("{} ({})", rule.name, rule.id);
    println!("  Category:    {}", rule.category);
    println!("  Tier:        {}", rule.risk);
    println!("  Why:         {}", rule.explain);
    if let Some(regenerate) = &rule.regenerate {
        println!("  Comes back:  {regenerate}");
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
    }
    if let Some(owner) = &rule.owner {
        println!("  Kept while:  {} is running", owner.app);
    }
    if let Some(days) = rule.idle_days {
        println!("  Kept until:  unchanged for {days} days, and since the computer last started");
    }
    println!("  Tests:       {}", rule.tests.len());
}
