//! Going from one level to another (`teleport -l <level> <marker>`): every level has a script world of its own, so
//! the hero and what the hero carries are moved across. The level left behind keeps its world as it was (it is
//! found again like that on the way back); what was carried out of it is gone from it.

use arx_script::{EntityId, EntityKind, ScriptWorld, StdHost};
use std::collections::HashMap;

/// Move the hero (life, skills, runes, quest log, gold, the pack and what is worn) and the game's global variables
/// from the world being left to the one being entered. Carried items become entities of the new world under their
/// own names (`key_base_0005` stays `key_base_0005`, which is how doors know their keys), with their script
/// variables; an item that was once carried out of the new level takes its old place in that world again.
pub fn carry_player(from: &mut ScriptWorld, from_host: &mut StdHost, to: &mut ScriptWorld, to_host: &mut StdHost) {
    to_host.player = from_host.player.clone();
    to.globals = from.globals.clone();
    // The hero's own script variables travel too.
    if let (Some(old), Some(new)) = (from.player, to.player) {
        let vars = from.entity(old).vars.clone();
        to.entity_mut(new).vars = vars;
    }
    // Everything carried or worn, each once.
    let mut items: Vec<EntityId> = from_host.player.inventory.clone();
    items.extend(from_host.player.equipped.iter().flatten().copied());
    items.sort_unstable();
    items.dedup();
    let mut moved: HashMap<EntityId, EntityId> = HashMap::new();
    for old in items {
        let e = from.entity(old).clone();
        let returning = to.find(&e.id_string, old).filter(|&n| to.entity(n).kind == EntityKind::Item && to.entity(n).class == e.class);
        let new = returning.unwrap_or_else(|| to.add_entity(EntityKind::Item, &e.class, e.instance, e.script.clone(), e.over_script.clone()));
        {
            let n = to.entity_mut(new);
            n.vars = e.vars;
            n.props = e.props;
            n.groups = e.groups;
            n.disabled_events = e.disabled_events;
            n.main_event = e.main_event;
            n.dead = false;
        }
        let state = from_host.state(old).cloned().unwrap_or_default();
        to_host.modify(new, |s| *s = state);
        // In the world left behind it no longer exists.
        from_host.modify(old, |s| {
            s.destroyed = true;
            s.in_inventory = false;
            s.equipped = false;
        });
        moved.insert(old, new);
    }
    let p = &mut to_host.player;
    for id in &mut p.inventory {
        *id = moved[id];
    }
    p.slots = std::mem::take(&mut p.slots).into_iter().map(|(id, slot)| (moved[&id], slot)).collect();
    for slot in &mut p.equipped {
        *slot = slot.map(|id| moved[&id]);
    }
    // The world left behind has a hero with empty hands, should anything there ask.
    from_host.player.inventory.clear();
    from_host.player.slots.clear();
    from_host.player.equipped = Default::default();
    to_host.player_entity = to.player;
    to_host.recompute_equipment();
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx_script::{Carry, EquipSlot, Script, Value};
    use std::sync::Arc;

    fn level() -> (ScriptWorld, StdHost, EntityId) {
        let mut w = ScriptWorld::new();
        let player = w.add_entity(EntityKind::Player, "graph/obj3d/interactive/player/player", 1, None, None);
        w.player = Some(player);
        (w, StdHost::new(), player)
    }

    fn item(w: &mut ScriptWorld, h: &mut StdHost, class: &str, instance: i32, src: &str) -> EntityId {
        let id = w.add_entity(EntityKind::Item, &format!("graph/obj3d/interactive/items/{class}"), instance, Some(Arc::new(Script::new(&src.chars().map(|c| c as u8).collect::<Vec<u8>>()))), None);
        w.send_init(h, id);
        id
    }

    #[test]
    fn the_hero_takes_the_pack_along_and_finds_things_back_on_return() {
        let (mut a, mut ha, _) = level();
        let (mut b, mut hb, _) = level();
        // In level A: a key, a stack of three apples and a sword in the hand; a rune learnt, some gold, a hurt hero.
        let key = item(&mut a, &mut ha, "quest_item/key_base/key_base", 5, "on init {\n set \u{a3}opens \"cell\"\n accept\n}");
        let apples = item(&mut a, &mut ha, "provisions/food_apple/food_apple", 2, "on init {\n playerstacksize 10\n accept\n}");
        let sword = item(&mut a, &mut ha, "weapons/sword/sword", 9, "on init {\n setobjecttype weapon\n setobjecttype 1h\n setequip damages 4\n accept\n}");
        ha.modify(apples, |s| s.count = 3);
        for i in [key, apples, sword] {
            assert_eq!(ha.carry(&a, i), Carry::Added);
        }
        ha.equip(&mut a, sword);
        ha.player.runes.insert("aam".into());
        ha.player.gold = 77;
        ha.player.life.current = 5.0;
        ha.player.quests.push("quest_one".into());
        a.globals.set("#escaped", Value::Int(1));
        let damages = ha.player.misc().damages;
        let apple_slot = ha.player.slots[&apples];
        // Something else lies in level A and stays there.
        let stone = item(&mut a, &mut ha, "special/stone/stone", 1, "on init {\n accept\n}");

        carry_player(&mut a, &mut ha, &mut b, &mut hb);

        // In B the hero is the same hero ...
        assert_eq!((hb.player.gold, hb.player.life.current, hb.player.runes.contains("aam"), hb.player.quests.len()), (77, 5.0, true, 1));
        assert_eq!(b.globals.get_int("#escaped"), 1);
        assert_eq!(hb.player.misc().damages, damages, "the sword still adds its damage");
        // ... with the same things under the same names, in the same places.
        let find = |w: &ScriptWorld, name: &str| w.find(name, 0).unwrap();
        let (key_b, apples_b, sword_b) = (find(&b, "key_base_0005"), find(&b, "food_apple_0002"), find(&b, "sword_0009"));
        assert!(hb.player.inventory.contains(&key_b) && hb.player.inventory.contains(&apples_b));
        assert_eq!(hb.player.equipped_in(EquipSlot::Weapon), Some(sword_b));
        assert_eq!((hb.state(apples_b).unwrap().count, hb.player.slots[&apples_b]), (3, apple_slot));
        assert_eq!(b.entity(key_b).vars.get_text("\u{a3}opens"), Some("cell"), "the key remembers what it opens");
        assert!(hb.state(key_b).unwrap().in_inventory && !hb.state(key_b).unwrap().destroyed);
        // A is left without them, and with what was not carried.
        assert!(ha.state(key).unwrap().destroyed && ha.state(sword).unwrap().destroyed && ha.state(stone).is_none_or(|s| !s.destroyed));
        assert!(ha.player.inventory.is_empty());

        // The hero drops the key in B, eats an apple, and goes back to A.
        hb.player.remove_item(key_b);
        hb.modify(key_b, |s| {
            s.in_inventory = false;
            s.hidden = false;
        });
        hb.modify(apples_b, |s| s.count = 2);
        let entities_in_a = a.entities.len();
        carry_player(&mut b, &mut hb, &mut a, &mut ha);
        // The apples and the sword are their old selves in A again (no doubles), the key stayed in B.
        assert_eq!(a.entities.len(), entities_in_a, "returning things take their old place");
        assert_eq!((find(&a, "food_apple_0002"), find(&a, "sword_0009")), (apples, sword));
        assert_eq!((ha.state(apples).unwrap().count, ha.state(apples).unwrap().destroyed), (2, false));
        assert_eq!(ha.player.equipped_in(EquipSlot::Weapon), Some(sword));
        assert!(!ha.player.inventory.contains(&key) && ha.state(key).unwrap().destroyed);
        assert!(!hb.state(key_b).unwrap().destroyed && !hb.state(key_b).unwrap().in_inventory, "the key lies in B");
    }
}
