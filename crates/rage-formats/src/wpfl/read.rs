//! Reading the rule dictionaries and effects of an effect list.

use super::*;

/// Rule offsets mapped to their index in [`EffectList::emitters`] and [`EffectList::particles`].
pub(super) struct RuleIndex {
    pub(super) emitters: std::collections::HashMap<u64, usize>,
    pub(super) particles: std::collections::HashMap<u64, usize>,
}

/// Reads every rule of a dictionary with `read`: the rules that read go into
/// `rules` (offset to index in the result), the others into `errors`.
pub(super) fn read_rules<T>(
    res: &Resource,
    dict: u64,
    read: fn(&Resource, u64) -> Result<T, ResourceError>,
    label: &str,
    (rules, errors): (&mut Vec<T>, &mut Vec<String>),
) -> Result<std::collections::HashMap<u64, usize>, ResourceError> {
    let mut index = std::collections::HashMap::new();
    for p in dictionary(res, dict)? {
        match read(res, p) {
            Ok(r) => {
                index.insert(p, rules.len());
                rules.push(r);
            }
            Err(e) => errors.push(format!("{label}: {e}")),
        }
    }
    Ok(index)
}

/// An emitter event: the emitter and particle rule it names, or `None` for
/// the other kinds of events.
fn effect_event(res: &Resource, ev: u64, index: &RuleIndex) -> Result<Option<EffectEvent>, ResourceError> {
    // Emitter events point at an emitter and a particle rule of
    // this list; other events have other fields there.
    let (em, pr) = (res.u32(ev + 0x48).map(u64::from), res.u32(ev + 0x4C).map(u64::from));
    let (Some(&emitter), Some(&particle)) =
        (em.ok().and_then(|e| index.emitters.get(&e)), pr.ok().and_then(|p| index.particles.get(&p)))
    else {
        return Ok(None);
    };
    let pair = |at: u64| -> Result<[f32; 2], ResourceError> { Ok([f32_at(res, at)?, f32_at(res, at + 4)?]) };
    Ok(Some(EffectEvent { emitter, particle, playback_rate: pair(ev + 0x20)?, zoom: pair(ev + 0x28)? }))
}

fn effect_rule(
    res: &Resource,
    p: u64,
    index: &RuleIndex,
    other_events: &mut usize,
) -> Result<EffectRule, ResourceError> {
    let mut rule = EffectRule { name: string_at(res, p + 0xF8)?, events: Vec::new() };
    let list = ptr(res, p + 0x130)?;
    let count = res.u16(p + 0x134)? as u64;
    if count > MAX_EVENTS {
        return Err(bad(format!("{}: implausible {count} events", rule.name)));
    }
    for i in 0..count {
        let ev = ptr(res, list + i * 4)?;
        match effect_event(res, ev, index)? {
            Some(event) => rule.events.push(event),
            None => *other_events += 1,
        }
    }
    Ok(rule)
}

pub(super) fn read_textures(res: &Resource, out: &mut EffectList) -> Result<(), ResourceError> {
    let txd = ptr(res, ROOT)?;
    if txd != 0 {
        match ytd::parse_gta4_at(res, txd) {
            Ok(t) => out.textures = Some(t),
            Err(e) => out.errors.push(format!("textures: {e}")),
        }
    }
    Ok(())
}

pub(super) fn read_models(res: &Resource, out: &mut EffectList) -> Result<(), ResourceError> {
    let models = ptr(res, ROOT + 0x08)?;
    if models != 0 {
        out.models = res.u16(models + 0x1C)? as usize;
    }
    Ok(())
}

pub(super) fn read_effects(res: &Resource, index: &RuleIndex, out: &mut EffectList) -> Result<(), ResourceError> {
    for p in dictionary(res, ptr(res, ROOT + 0x18)?)? {
        match effect_rule(res, p, index, &mut out.other_events) {
            Ok(r) => out.effects.push(r),
            Err(e) => out.errors.push(format!("effect rule: {e}")),
        }
    }
    Ok(())
}
