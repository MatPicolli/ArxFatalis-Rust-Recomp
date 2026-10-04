//! What the player can do with items: pick them up, use them, combine them with something, put them back.
//! The state lives in [`StdHost::player`] and the entity states; items react through their scripts
//! (`inventoryin`, `inventoryuse`, `combine`, `inventoryout`).

use arx_script::{Carry, EntityId, EntityKind, ScriptResult, ScriptWorld, StdHost};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickUp {
    /// Now in the inventory as its own entry.
    Added,
    /// Merged into the stack of the same kind the player already carries (its entity).
    Stacked(EntityId),
    /// Gold: this many coins went to the purse.
    Gold(u64),
    /// Could not be picked up, and why.
    Refused(&'static str),
}

fn is_carried(host: &StdHost, id: EntityId) -> bool {
    host.player.inventory.contains(&id)
}

/// Pick `item` up from the world.
pub fn pick_up(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, item: EntityId) -> PickUp {
    if world.entity(item).kind != EntityKind::Item {
        return PickUp::Refused("not an item");
    }
    let st = host.state(item).cloned().unwrap_or_default();
    if st.hidden || st.destroyed || st.in_inventory {
        return PickUp::Refused("not in the world");
    }
    if !st.interactive {
        return PickUp::Refused("cannot be touched");
    }

    carried(world, host, player, item)
}

/// What happened when `item` was given to the player: tell the item if it now sits in the inventory.
fn carried(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, item: EntityId) -> PickUp {
    match host.carry(world, item) {
        Carry::Stacked(target) => PickUp::Stacked(target),
        Carry::Gold(coins) => PickUp::Gold(coins),
        Carry::Full => PickUp::Refused("no room"),
        Carry::Added => {
            world.send_event(host, Some(player), item, "inventoryin", Vec::new());
            host.prune_inventory();
            PickUp::Added
        }
    }
}

/// Use a carried item (eat it, read it, ...): its `inventoryuse` event decides what happens.
pub fn use_item(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, item: EntityId) -> bool {
    if !is_carried(host, item) {
        return false;
    }
    world.send_event(host, Some(player), item, "inventoryuse", Vec::new());
    host.prune_inventory();
    true
}

/// Use `source` on `target` (a key on a door, an item on another item): `target` gets a `combine` event naming
/// `source`. Returns what the target's script answered.
pub fn combine(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, source: EntityId, target: EntityId) -> ScriptResult {
    let name = world.entity(source).id_string.clone();
    let result = world.send_event(host, Some(player), target, "combine", vec![name]);
    host.prune_inventory();
    result
}

/// Does `entity` hold things the player can look into (a chest, a corpse)?
pub fn is_container(world: &ScriptWorld, host: &StdHost, entity: EntityId) -> bool {
    host.containers.contains_key(&entity) && world.entity(entity).kind != EntityKind::Npc
}

/// Open `container`. Its `inventory2_open` script decides: a locked chest refuses (and complains). Returns whether
/// it is open now. Whatever was open before is closed first.
pub fn open_container(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, container: EntityId) -> bool {
    if !host.containers.contains_key(&container) {
        return false;
    }
    close_container(world, host, player);
    if world.send_event(host, Some(player), container, "inventory2_open", Vec::new()) == ScriptResult::Refuse {
        return false;
    }
    host.open_container = Some(container);
    true
}

/// Close the open container, letting its script know (a chest lid closes).
pub fn close_container(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId) {
    if let Some(c) = host.open_container.take() {
        world.send_event(host, Some(player), c, "inventory2_close", Vec::new());
    }
}

/// Take one item out of an open container (a chest, a corpse) into the player's inventory.
pub fn take_from_container(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, container: EntityId, item: EntityId) -> Option<PickUp> {
    let at = host.containers.get(&container)?.iter().position(|&i| i == item)?;
    let result = carried(world, host, player, item);
    if result != PickUp::Refused("no room") {
        host.containers.get_mut(&container)?.remove(at);
    }
    Some(result)
}

/// Put a carried item into the open container (the player putting something away).
pub fn store_in_container(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, container: EntityId, item: EntityId) -> bool {
    if !host.containers.contains_key(&container) || !host.player.inventory.contains(&item) {
        return false;
    }
    host.player.remove_item(item);
    host.containers.entry(container).or_default().push(item);
    world.send_event(host, Some(player), item, "inventoryout", Vec::new());
    true
}

/// Take everything out of a container; returns how many entries were taken.
pub fn take_all(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, container: EntityId) -> usize {
    let items = host.containers.get(&container).cloned().unwrap_or_default();
    items.iter().filter(|&&i| matches!(take_from_container(world, host, player, container, i), Some(r) if r != PickUp::Refused("no room"))).count()
}

/// Put a carried item back into the world at `pos` (Arx coordinates).
pub fn drop_item(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, item: EntityId, pos: [f32; 3]) -> bool {
    if !host.player.inventory.contains(&item) {
        return false;
    }
    host.player.remove_item(item);
    world.entity_mut(item).pos = pos;
    host.modify(item, |s| {
        s.in_inventory = false;
        s.hidden = false;
        s.collision = true;
        s.moved_to = Some(pos);
    });
    host.note_dropped(item);
    world.send_event(host, Some(player), item, "inventoryout", Vec::new());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use arx_script::Script;
    use std::sync::Arc;

    const PIE: &str = "on init {\n playerstacksize 3\n setfood 14\n accept\n}\non inventoryin {\n herosay got_pie\n accept\n}\non inventoryuse {\n specialfx heal 4\n eatme\n accept\n}\non inventoryout {\n herosay dropped\n accept\n}";
    const KEY: &str = "on init {\n setname [description_key]\n accept\n}";
    // A locked door opens its lock when it is combined with its key.
    const DOOR: &str = "on init {\n set £key \"key_base_0005\"\n set §unlock 0\n accept\n}\non combine {\n if (^$param1 isin £key) {\n  set §unlock 1\n  refuse\n }\n accept\n}";

    /// Script files are Latin-1: `§` is one byte.
    fn script(src: &str) -> Arc<Script> {
        let bytes: Vec<u8> = src.chars().map(|c| c as u8).collect();
        Arc::new(Script::new(&bytes))
    }

    fn scene() -> (ScriptWorld, StdHost, EntityId) {
        let mut w = ScriptWorld::new();
        let player = w.add_entity(EntityKind::Player, "graph/obj3d/interactive/npc/player/player", 1, None, None);
        w.player = Some(player);
        (w, StdHost::new(), player)
    }

    fn item(w: &mut ScriptWorld, h: &mut StdHost, class: &str, instance: i32, src: &str) -> EntityId {
        let id = w.add_entity(EntityKind::Item, class, instance, Some(script(src)), None);
        w.send_init(h, id);
        id
    }

    #[test]
    fn picking_up_takes_an_item_out_of_the_world_and_tells_it() {
        let (mut w, mut h, player) = scene();
        let pie = item(&mut w, &mut h, "items/provisions/applepie/applepie", 1, PIE);
        assert_eq!(pick_up(&mut w, &mut h, player, pie), PickUp::Added);
        assert_eq!(h.player.inventory, [pie]);
        let st = h.state(pie).unwrap();
        assert!(st.hidden && st.in_inventory && !st.collision);
        assert_eq!(h.take_messages(), ["got_pie"], "inventoryin ran");
        assert_eq!(pick_up(&mut w, &mut h, player, pie), PickUp::Refused("not in the world"));
        let door = w.add_entity(EntityKind::Fix, "fix_inter/door/door", 1, None, None);
        assert_eq!(pick_up(&mut w, &mut h, player, door), PickUp::Refused("not an item"));
    }

    #[test]
    fn items_of_a_kind_share_a_stack_up_to_the_stack_size() {
        let (mut w, mut h, player) = scene();
        let pies: Vec<_> = (1..=4).map(|n| item(&mut w, &mut h, "items/provisions/applepie/applepie", n, PIE)).collect();
        assert_eq!(pick_up(&mut w, &mut h, player, pies[0]), PickUp::Added);
        assert_eq!(pick_up(&mut w, &mut h, player, pies[1]), PickUp::Stacked(pies[0]));
        assert_eq!(pick_up(&mut w, &mut h, player, pies[2]), PickUp::Stacked(pies[0]));
        assert_eq!(h.state(pies[0]).unwrap().count, 3);
        // The stack is full (3): the next one starts another.
        assert_eq!(pick_up(&mut w, &mut h, player, pies[3]), PickUp::Added);
        assert_eq!(h.player.inventory, [pies[0], pies[3]]);
        assert!(h.state(pies[1]).unwrap().destroyed, "merged items leave the world");
        // A different kind never merges (keys do not stack).
        let key = item(&mut w, &mut h, "items/keys/key_base/key_base", 5, KEY);
        assert_eq!(pick_up(&mut w, &mut h, player, key), PickUp::Added);
    }

    #[test]
    fn using_an_item_applies_its_effect_and_consumes_it() {
        let (mut w, mut h, player) = scene();
        let a = item(&mut w, &mut h, "items/provisions/applepie/applepie", 1, PIE);
        let b = item(&mut w, &mut h, "items/provisions/applepie/applepie", 2, PIE);
        pick_up(&mut w, &mut h, player, a);
        pick_up(&mut w, &mut h, player, b);
        h.player.life.current = 2.0;
        assert!(use_item(&mut w, &mut h, player, a));
        assert_eq!(h.player.life.current, 6.0);
        assert_eq!(h.state(a).unwrap().count, 1);
        assert!(use_item(&mut w, &mut h, player, a));
        assert_eq!(h.player.life.current, 10.0);
        assert!(h.player.inventory.is_empty(), "the last one was eaten");
        assert!(!use_item(&mut w, &mut h, player, a), "cannot use what is not carried");
    }

    #[test]
    fn a_key_combined_with_its_door_unlocks_it_and_other_keys_do_not() {
        let (mut w, mut h, player) = scene();
        let key = item(&mut w, &mut h, "items/keys/key_base/key_base", 5, KEY);
        let other = item(&mut w, &mut h, "items/keys/key_base/key_base", 6, KEY);
        let door = w.add_entity(EntityKind::Fix, "fix_inter/door/door", 1, Some(script(DOOR)), None);
        w.send_init(&mut h, door);
        assert_eq!(w.entity(key).id_string, "key_base_0005");
        combine(&mut w, &mut h, player, other, door);
        assert_eq!(w.entity(door).vars.get_int("\u{a7}unlock"), 0);
        combine(&mut w, &mut h, player, key, door);
        assert_eq!(w.entity(door).vars.get_int("\u{a7}unlock"), 1);
    }

    #[test]
    fn items_can_be_put_away_in_a_chest() {
        let (mut w, mut h, player) = scene();
        let chest = w.add_entity(EntityKind::Fix, "fix_inter/chest/chest", 1, Some(script("on init {\n inventory create\n accept\n}")), None);
        w.send_init(&mut h, chest);
        let pie = item(&mut w, &mut h, "items/provisions/applepie/applepie", 1, PIE);
        assert!(!store_in_container(&mut w, &mut h, player, chest, pie), "not carried yet");
        pick_up(&mut w, &mut h, player, pie);
        assert!(store_in_container(&mut w, &mut h, player, chest, pie));
        assert!(h.player.inventory.is_empty() && h.player.slots.is_empty());
        assert_eq!(h.containers[&chest], [pie]);
        assert_eq!(take_from_container(&mut w, &mut h, player, chest, pie), Some(PickUp::Added));
    }

    #[test]
    fn dropping_puts_the_item_back_in_the_world_where_asked() {
        let (mut w, mut h, player) = scene();
        let pie = item(&mut w, &mut h, "items/provisions/applepie/applepie", 1, PIE);
        pick_up(&mut w, &mut h, player, pie);
        h.take_messages();
        assert!(drop_item(&mut w, &mut h, player, pie, [10.0, 20.0, 30.0]));
        let st = h.state(pie).unwrap();
        assert!(!st.hidden && !st.in_inventory);
        assert_eq!(st.moved_to, Some([10.0, 20.0, 30.0]));
        assert_eq!(h.take_dropped(), [pie], "the renderer is told to give it a model");
        assert!(h.take_dropped().is_empty());
        assert_eq!(w.entity(pie).pos, [10.0, 20.0, 30.0]);
        assert!(h.player.inventory.is_empty());
        assert_eq!(h.take_messages(), ["dropped"]);
        assert!(!drop_item(&mut w, &mut h, player, pie, [0.0; 3]));
        assert_eq!(pick_up(&mut w, &mut h, player, pie), PickUp::Added, "it can be picked up again");
    }

    #[test]
    fn chests_hold_items_that_can_be_taken_out() {
        let (mut w, mut h, player) = scene();
        let class_scripts: std::collections::HashMap<String, Arc<Script>> = [
            ("graph/obj3d/interactive/items/provisions/applepie/applepie".to_owned(), script(PIE)),
            ("graph/obj3d/interactive/items/gold_coin/gold_coin".to_owned(), script("on init {\n playerstacksize 9999\n accept\n}")),
        ]
        .into();
        h.set_script_loader(Box::new(move |class| class_scripts.get(class).cloned()));
        let chest_src = "on init {\n inventory create\n inventory add \"provisions\\\\applepie\\\\applepie\"\n inventory addmulti gold_coin\\gold_coin 25\n inventory add provisions\\nothing\\nothing\n set \u{a7}locked 1\n accept\n}\non inventory2_open {\n if (\u{a7}locked == 1) {\n  herosay locked\n  refuse\n }\n accept\n}\non inventory2_close {\n herosay lid_closed\n accept\n}\non unlock {\n set \u{a7}locked 0\n accept\n}";
        let chest = w.add_entity(EntityKind::Fix, "fix_inter/chest/chest", 1, Some(script(chest_src)), None);
        w.send_init(&mut h, chest);
        let held = h.containers.get(&chest).expect("inventory create made a container").clone();
        assert_eq!(held.len(), 2, "the unknown item class could not be added");
        assert_eq!(h.state(held[1]).map(|s| s.count), Some(25));
        assert!(h.state(held[0]).unwrap().hidden, "contents are not in the world");
        assert!(!w.stats.warnings.is_empty());
        assert!(is_container(&w, &h, chest));
        assert!(!open_container(&mut w, &mut h, player, chest), "a locked chest refuses");
        assert_eq!((h.open_container, h.take_messages()), (None, vec!["locked".to_owned()]));
        w.send_event(&mut h, Some(player), chest, "unlock", vec![]);
        assert!(open_container(&mut w, &mut h, player, chest));
        assert_eq!(h.open_container, Some(chest));

        assert_eq!(take_from_container(&mut w, &mut h, player, chest, held[0]), Some(PickUp::Added));
        assert_eq!(h.player.inventory, [held[0]]);
        assert_eq!(h.containers[&chest], [held[1]]);
        assert_eq!(take_from_container(&mut w, &mut h, player, chest, held[0]), None, "already taken");
        assert_eq!(take_all(&mut w, &mut h, player, chest), 1);
        assert!(h.containers[&chest].is_empty());
        assert_eq!(h.player.gold, 25, "coins go to the purse, not the grid");
        assert_eq!(h.player.inventory, [held[0]]);
        h.take_messages();
        close_container(&mut w, &mut h, player);
        assert_eq!((h.open_container, h.take_messages()), (None, vec!["lid_closed".to_owned()]));
    }

    #[test]
    fn items_created_for_the_player_stack_with_what_is_carried() {
        let (mut w, mut h, player) = scene();
        let scripts: std::collections::HashMap<String, Arc<Script>> =
            [("graph/obj3d/interactive/items/provisions/applepie/applepie".to_owned(), script(PIE))].into();
        h.set_script_loader(Box::new(move |class| scripts.get(class).cloned()));
        let giver = w.add_entity(
            EntityKind::Fix,
            "fix_inter/giver/giver",
            1,
            Some(script("on action {\n inventory playeradd provisions\\applepie\\applepie\n inventory playeraddmulti provisions\\applepie\\applepie 2\n accept\n}")),
            None,
        );
        w.send_event(&mut h, Some(player), giver, "action", vec![]);
        assert_eq!(h.player.inventory.len(), 1, "both pies share one stack");
        assert_eq!(h.state(h.player.inventory[0]).unwrap().count, 3);
        assert_eq!(h.take_messages(), ["got_pie"], "inventoryin ran for the new entry only");
    }

    #[test]
    fn a_full_inventory_refuses_items_and_leaves_them_where_they_were() {
        let (mut w, mut h, player) = scene();
        let mut keys = Vec::new();
        for n in 1..=(arx_script::BAG_WIDTH as i32 * arx_script::BAG_HEIGHT as i32 + 1) {
            keys.push(item(&mut w, &mut h, "items/keys/key_base/key_base", n, KEY));
        }
        let (last, rest) = keys.split_last().unwrap();
        for &k in rest {
            assert_eq!(pick_up(&mut w, &mut h, player, k), PickUp::Added);
        }
        assert_eq!(h.player.inventory.len(), 48);
        assert_eq!(pick_up(&mut w, &mut h, player, *last), PickUp::Refused("no room"));
        assert!(!h.state(*last).unwrap().hidden, "the item stays in the world");
        h.player.bags = 2;
        assert_eq!(pick_up(&mut w, &mut h, player, *last), PickUp::Added, "a new bag makes room");
        assert_eq!(h.player.slots[last].bag, 1);
    }

    #[test]
    fn big_icons_take_several_slots_and_dropping_frees_them() {
        let (mut w, mut h, player) = scene();
        h.set_icon_size(Box::new(|class| if class.contains("sword") { Some((64, 96)) } else { Some((32, 32)) }));
        let sword = item(&mut w, &mut h, "items/weapons/sword/sword", 1, KEY);
        let key = item(&mut w, &mut h, "items/keys/key_base/key_base", 1, KEY);
        assert_eq!(pick_up(&mut w, &mut h, player, sword), PickUp::Added);
        let s = h.player.slots[&sword];
        assert_eq!((s.w, s.h), (2, 3), "a 64x96 icon is 2x3 slots");
        assert_eq!(pick_up(&mut w, &mut h, player, key), PickUp::Added);
        assert_eq!((h.player.slots[&key].x, h.player.slots[&key].y), (2, 0), "next to the sword");
        assert!(drop_item(&mut w, &mut h, player, sword, [0.0; 3]));
        assert!(h.player.slots.get(&sword).is_none());
        assert!(h.player.area_free(0, 0, 0, 2, 3, None));
    }

    #[test]
    fn gold_goes_to_the_purse() {
        let (mut w, mut h, player) = scene();
        let coins = item(&mut w, &mut h, "items/jewelry/gold_coin/gold_coin", 1, "on init {
 set_price 1
 playerstacksize 999
 accept
}");
        h.modify(coins, |s| s.count = 40);
        assert_eq!(pick_up(&mut w, &mut h, player, coins), PickUp::Gold(40));
        assert_eq!(h.player.gold, 40);
        assert!(h.player.inventory.is_empty() && h.state(coins).unwrap().destroyed);
    }
}
