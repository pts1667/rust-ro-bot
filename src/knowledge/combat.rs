//! A rough pre-renewal fight model: how long a monster takes to kill and how much it hurts. It only has to
//! rank monsters and keep the bot away from the ones that would kill it, so it errs on the careful side.

use super::db::MobData;

/// What the fight model needs to know about the bot.
#[derive(Debug, Clone, Default)]
pub struct Fighter {
    pub level: u32,
    pub max_hp: u32,
    pub str: u32,
    pub agi: u32,
    pub vit: u32,
    pub dex: u32,
    pub luk: u32,
    pub weapon_atk: u32,
    pub armor_def: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assessment {
    /// Seconds the bot needs to kill the monster.
    pub kill_secs: f32,
    /// HP the bot is expected to lose in that fight.
    pub damage_taken: f32,
    /// `damage_taken / max_hp`.
    pub danger: f32,
    /// Base experience per second of fighting, with some walking time.
    pub exp_per_sec: f32,
}

/// Fights expected to cost more than this share of max HP are not picked.
pub const SAFE_DANGER: f32 = 0.6;

impl Fighter {
    fn attack(&self) -> f32 {
        let s = self.str as f32;
        self.weapon_atk as f32
            + s
            + (s / 10.0).floor().powi(2)
            + self.dex as f32 / 5.0
            + self.luk as f32 / 5.0
    }

    /// Seconds between two attacks of the bot. A Novice with no AGI swings about every 1.4 s.
    fn attack_delay(&self) -> f32 {
        (1.4 * (1.0 - self.agi as f32 / 250.0 - self.dex as f32 / 1000.0)).max(0.3)
    }

    pub fn assess(&self, mob: &MobData) -> Assessment {
        let def_factor = (100.0 - mob.def.min(95) as f32) / 100.0;
        let my_damage = (self.attack() * def_factor).max(1.0);
        let my_hit = ((80 + self.level + self.dex) as f32 - (mob.level + mob.agi) as f32)
            .clamp(5.0, 95.0)
            / 100.0;
        let hits = (mob.hp.max(1) as f32 / (my_damage * my_hit)).ceil();
        let kill_secs = hits * self.attack_delay();

        let mob_avg = (mob.atk1 + mob.atk2.max(mob.atk1)) as f32 / 2.0;
        let mob_damage = (mob_avg - (self.vit + self.armor_def) as f32).max(1.0);
        let mob_hit = ((80 + mob.level + mob.dex) as f32 - (self.level + self.agi) as f32)
            .clamp(5.0, 95.0)
            / 100.0;
        let mob_delay = (mob.atk_delay.max(300) as f32) / 1000.0;
        let damage_taken = kill_secs / mob_delay * mob_damage * mob_hit;

        let danger = damage_taken / self.max_hp.max(1) as f32;
        let exp_per_sec = mob.exp as f32 / (kill_secs + 4.0);
        Assessment {
            kill_secs,
            damage_taken,
            danger,
            exp_per_sec,
        }
    }

    /// Whether two of the monster's hardest hits kill the bot from full HP.
    pub fn lethal_in_two(&self, mob: &MobData) -> bool {
        let hit = mob
            .atk2
            .max(mob.atk1)
            .saturating_sub(self.vit + self.armor_def);
        2 * hit >= self.max_hp
    }

    /// Whether the bot should start a fight with this monster.
    pub fn can_fight(&self, mob: &MobData) -> bool {
        !mob.boss() && self.assess(mob).danger < SAFE_DANGER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn novice() -> Fighter {
        Fighter {
            level: 1,
            max_hp: 40,
            str: 1,
            agi: 1,
            vit: 1,
            dex: 1,
            luk: 1,
            weapon_atk: 17,
            armor_def: 1,
        }
    }

    fn mob(
        name: &str,
        level: u32,
        hp: u32,
        atk: (u32, u32),
        def: u32,
        delay: u32,
        exp: u32,
    ) -> MobData {
        MobData {
            name_english: name.into(),
            level,
            hp,
            atk1: atk.0,
            atk2: atk.1,
            def,
            atk_delay: delay,
            exp,
            ..Default::default()
        }
    }

    #[test]
    fn novice_can_fight_poring_not_wolf() {
        let poring = mob("Poring", 1, 50, (7, 10), 0, 1872, 2);
        let wolf = mob("Wolf", 25, 919, (37, 46), 0, 1054, 329);
        let me = novice();
        assert!(me.can_fight(&poring), "{:?}", me.assess(&poring));
        assert!(!me.can_fight(&wolf), "{:?}", me.assess(&wolf));
    }

    #[test]
    fn bosses_are_never_fought() {
        let mut poring = mob("Poring", 1, 50, (7, 10), 0, 1872, 2);
        poring.mvp = true;
        assert!(!novice().can_fight(&poring));
    }
}
