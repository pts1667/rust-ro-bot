//! What party members say to each other, in plain words a player would type and read. The bots say these lines
//! exactly; a person in the party can say them too ("follow me", "stay here", "heal please").

/// Something said on the party channel, by `from`.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub from: String,
    pub said: Said,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Said {
    /// The speaker hunts on this map: "hunting at prt_fild08".
    Hunting(String),
    /// Come to the speaker: "follow me".
    FollowMe,
    /// Stop following, hold the position: "stay here".
    Stay,
    /// "level 12/9 Swordman": base level, job level and job.
    Level { base: u32, job: u32, job_name: String },
    /// The speaker wants a heal: "heal please".
    NeedHeal,
    /// The speaker has no healing items left: "out of potions".
    NeedPotions,
    /// The speaker hands potions to this member, nobody else needs to: "sending potions to Bravo".
    SendingPotions(String),
    /// The speaker is busy elsewhere for a while: "brb, selling loot".
    Away(String),
    /// The speaker is back from an errand: "back".
    Back,
    /// Anything else.
    Other(String),
}

impl Said {
    /// The words for it, as the bots say them.
    pub fn text(&self) -> String {
        match self {
            Said::Hunting(map) => format!("hunting at {map}"),
            Said::FollowMe => "follow me".into(),
            Said::Stay => "stay here".into(),
            Said::Level { base, job, job_name } => format!("level {base}/{job} {job_name}"),
            Said::NeedHeal => "heal please".into(),
            Said::NeedPotions => "out of potions".into(),
            Said::SendingPotions(to) => format!("sending potions to {to}"),
            Said::Away(reason) => format!("brb, {reason}"),
            Said::Back => "back".into(),
            Said::Other(text) => text.clone(),
        }
    }
}

/// Reads a party message (`Name : text`).
pub fn parse(message: &str) -> Option<Line> {
    let (from, text) = message.split_once(" : ")?;
    let from = from.trim().to_owned();
    let text = text.trim();
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let said = if let Some(map) = lower.strip_prefix("hunting at ").or_else(|| lower.strip_prefix("hunting on ")) {
        Said::Hunting(map.trim().trim_end_matches(['.', '!']).to_owned())
    } else if lower.starts_with("follow me") || lower == "come here" || lower == "regroup" {
        Said::FollowMe
    } else if lower.starts_with("stay") || lower == "wait here" || lower == "hold" {
        Said::Stay
    } else if words.first() == Some(&"level") && words.len() >= 2 {
        let (base, job) = words[1].split_once('/').unwrap_or((words[1], "0"));
        match (base.parse(), job.parse()) {
            (Ok(base), Ok(job)) => {
                Said::Level { base, job, job_name: text.split_whitespace().nth(2).unwrap_or_default().to_owned() }
            }
            _ => Said::Other(text.to_owned()),
        }
    } else if lower.starts_with("heal") || lower.contains("need heal") || lower.contains("heal me") {
        Said::NeedHeal
    } else if let Some(to) = lower.strip_prefix("sending potions to ").or_else(|| lower.strip_prefix("giving potions to ")) {
        Said::SendingPotions(to.trim().trim_end_matches(['.', '!']).to_owned())
    } else if lower.contains("out of potions") || lower.contains("need potions") || lower.contains("no potions") {
        Said::NeedPotions
    } else if let Some(reason) = lower.strip_prefix("brb") {
        Said::Away(reason.trim_start_matches([',', ' ', ':']).trim().to_owned())
    } else if lower == "back" || lower == "i'm back" || lower == "im back" {
        Said::Back
    } else {
        Said::Other(text.to_owned())
    };
    Some(Line { from, said })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(message: &str) -> Said {
        parse(message).unwrap().said
    }

    #[test]
    fn the_lines_the_bots_say_read_back_the_same() {
        let all = [
            Said::Hunting("prt_fild08".into()),
            Said::FollowMe,
            Said::Stay,
            Said::Level { base: 12, job: 9, job_name: "Swordman".into() },
            Said::NeedHeal,
            Said::NeedPotions,
            Said::SendingPotions("bravo".into()),
            Said::Away("selling loot".into()),
            Said::Back,
        ];
        for line in all {
            assert_eq!(said(&format!("Alpha : {}", line.text())), line);
        }
        assert_eq!(parse("Alpha : back").unwrap().from, "Alpha");
    }

    #[test]
    fn people_can_say_them_in_their_own_words() {
        assert_eq!(said("Tom : Follow me!"), Said::FollowMe);
        assert_eq!(said("Tom : heal me pls"), Said::NeedHeal);
        assert_eq!(said("Tom : Hunting at moc_fild01."), Said::Hunting("moc_fild01".into()));
        assert_eq!(said("Tom : I need potions"), Said::NeedPotions);
        assert_eq!(said("Tom : level up!"), Said::Other("level up!".into()));
        assert!(parse("no speaker here").is_none());
    }
}
