//! What the player can do with items: pick them up, use them, combine them with something, put them back.
//! The state lives in [`StdHost::player`] and the entity states; items react through their scripts
//! (`inventoryin`, `inventoryuse`, `combine`, `inventoryout`).

use arx_script::{EntityId, EntityKind, ScriptResult, ScriptWorld, StdHost};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickUp {
    /// Now in the inventory as its own entry.
    Added,
    /// Merged into the stack of the same kind the player already carries (its entity).
    Stacked(EntityId),
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

    // Items of the same kind share a stack, up to `playerstacksize`.
    let class = world.entity(item).class.clone();
    let target = host.player.inventory.iter().copied().find(|&i| {
        world.entity(i).class == class && host.state(i).is_some_and(|t| t.stack_size > 1 && t.count < t.stack_size)
    });
    let mut remaining = st.count;
    if let Some(target) = target {
        let t = host.state(target).expect("carried items have a state");
        let moved = remaining.min(t.stack_size - t.count);
        host.modify(target, |t| t.count += moved);
        remaining -= moved;
        if remaining == 0 {
            host.modify(item, |s| {
                s.count = 0;
                s.destroyed = true;
                s.collision = false;
            });
            return PickUp::Stacked(target);
        }
    }

    host.modify(item, |s| {
        s.count = remaining;
        s.in_inventory = true;
        s.hidden = true;
        s.collision = false;
        s.moved_to = None;
    });
    host.player.inventory.push(item);
    world.send_event(host, Some(player), item, "inventoryin", Vec::new());
    host.prune_inventory();
    PickUp::Added
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

/// Put a carried item back into the world at `pos` (Arx coordinates).
pub fn drop_item(world: &mut ScriptWorld, host: &mut StdHost, player: EntityId, item: EntityId, pos: [f32; 3]) -> bool {
    let Some(i) = host.player.inventory.iter().position(|&e| e == item) else { return false };
    host.player.inventory.remove(i);
    world.entity_mut(item).pos = pos;
    host.modify(item, |s| {
        s.in_inventory = false;
        s.hidden = false;
        s.moved_to = Some(pos);
    });
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
    fn dropping_puts_the_item_back_in_the_world_where_asked() {
        let (mut w, mut h, player) = scene();
        let pie = item(&mut w, &mut h, "items/provisions/applepie/applepie", 1, PIE);
        pick_up(&mut w, &mut h, player, pie);
        h.take_messages();
        assert!(drop_item(&mut w, &mut h, player, pie, [10.0, 20.0, 30.0]));
        let st = h.state(pie).unwrap();
        assert!(!st.hidden && !st.in_inventory);
        assert_eq!(st.moved_to, Some([10.0, 20.0, 30.0]));
        assert_eq!(w.entity(pie).pos, [10.0, 20.0, 30.0]);
        assert!(h.player.inventory.is_empty());
        assert_eq!(h.take_messages(), ["dropped"]);
        assert!(!drop_item(&mut w, &mut h, player, pie, [0.0; 3]));
        assert_eq!(pick_up(&mut w, &mut h, player, pie), PickUp::Added, "it can be picked up again");
    }
}
