//! `debug_info_item` state machine (`DebugInfoParser`/`DexLocalVar`): source line positions and
//! local variable ranges (incl. method parameter names).

use jadx_input_api::{DebugInfo, LocalVar};

use crate::error::Result;
use crate::file::DexFile;
use crate::reader::Reader;

const DBG_END_SEQUENCE: u8 = 0x00;
const DBG_ADVANCE_PC: u8 = 0x01;
const DBG_ADVANCE_LINE: u8 = 0x02;
const DBG_START_LOCAL: u8 = 0x03;
const DBG_START_LOCAL_EXTENDED: u8 = 0x04;
const DBG_END_LOCAL: u8 = 0x05;
const DBG_RESTART_LOCAL: u8 = 0x06;
const DBG_SET_PROLOGUE_END: u8 = 0x07;
const DBG_SET_EPILOGUE_BEGIN: u8 = 0x08;
const DBG_SET_FILE: u8 = 0x09;
const DBG_FIRST_SPECIAL: i64 = 0x0a;
const DBG_LINE_BASE: i64 = -4;
const DBG_LINE_RANGE: i64 = 15;

/// `DebugInfoParser.initMthArgs`: register of each explicit (non-`this`) argument, counted
/// backwards from `registers_size` (the arguments are the last `registers_size - this` registers
/// of the frame, in declaration order). `arg_reg_widths[i]` is the register width of argument `i`
/// (`J`/`D` take two registers, everything else one).
fn arg_regs(registers_size: u32, arg_reg_widths: &[u32]) -> Vec<u32> {
    let mut regs = vec![0u32; arg_reg_widths.len()];
    let mut reg = registers_size;
    for (i, w) in arg_reg_widths.iter().enumerate().rev() {
        reg = reg.saturating_sub(*w);
        regs[i] = reg;
    }
    regs
}

#[derive(Clone)]
struct ActiveVar {
    reg: u32,
    name: String,
    ty: String,
    signature: Option<String>,
    start: i32,
    end_set: bool,
}

impl DexFile {
    /// `DebugInfoParser.process`: source line positions (in execution order) and local variable
    /// ranges (incl. parameter names) of the `debug_info_item` at `off`.
    pub fn debug_info(
        &self,
        off: u32,
        code_size: u32,
        registers_size: u32,
        arg_reg_widths: &[u32],
    ) -> Result<DebugInfo> {
        let mut out = DebugInfo::default();
        if off == 0 {
            return Ok(out);
        }
        let arg_regs = arg_regs(registers_size, arg_reg_widths);
        let argc = arg_reg_widths.len() as u32;
        let mut r = Reader::new(self.bytes(), off as usize);
        let mut line = r.uleb128()? as i64;
        let params = r.uleb128()?;
        // `locals[reg]`: currently open variable of that register, if any.
        let mut locals: Vec<Option<ActiveVar>> = vec![None; registers_size as usize];
        let mut result: Vec<LocalVar> = Vec::new();
        let mut addr: u32 = 0;
        let code_last = code_size.saturating_sub(1);

        let start_var = |locals: &mut Vec<Option<ActiveVar>>,
                         result: &mut Vec<LocalVar>,
                         mut v: ActiveVar,
                         addr: i32| {
            let reg = v.reg as usize;
            if reg < locals.len() {
                if let Some(prev) = locals[reg].take() {
                    end_var(result, prev, addr);
                }
                v.start = addr;
                v.end_set = false;
                locals[reg] = Some(v);
            }
        };

        for i in 0..params {
            let name_id = r.uleb128p1()?;
            let name = if name_id >= 0 {
                self.opt_string(name_id as u32)?
            } else {
                None
            };
            if let Some(name) = name.filter(|_| i < argc) {
                let v = ActiveVar {
                    reg: arg_regs[i as usize],
                    name,
                    ty: String::new(),
                    signature: None,
                    start: 0,
                    end_set: false,
                };
                start_var(&mut locals, &mut result, v, 0);
                if let Some(slot) = locals
                    .get_mut(arg_regs[i as usize] as usize)
                    .and_then(|s| s.as_mut())
                {
                    slot.start = -1;
                }
            }
        }

        loop {
            let op = r.u8()?;
            match op {
                DBG_END_SEQUENCE => break,
                DBG_ADVANCE_PC => {
                    let inc = r.uleb128()?;
                    addr = addr.saturating_add(inc).min(code_last);
                }
                DBG_ADVANCE_LINE => {
                    line += i64::from(r.sleb128()?);
                }
                DBG_START_LOCAL => {
                    let reg = r.uleb128()?;
                    let name_id = r.uleb128p1()?;
                    let type_id = r.uleb128p1()?;
                    let name = if name_id >= 0 {
                        self.opt_string(name_id as u32)?
                    } else {
                        None
                    };
                    let ty = if type_id >= 0 {
                        self.opt_type_descriptor(type_id as u32)?
                    } else {
                        None
                    };
                    let v = ActiveVar {
                        reg,
                        name: name.unwrap_or_default(),
                        ty: ty.unwrap_or_default(),
                        signature: None,
                        start: 0,
                        end_set: false,
                    };
                    start_var(&mut locals, &mut result, v, addr as i32);
                }
                DBG_START_LOCAL_EXTENDED => {
                    let reg = r.uleb128()?;
                    let name_id = r.uleb128p1()?;
                    let type_id = r.uleb128p1()?;
                    let sign_id = r.uleb128p1()?;
                    let name = if name_id >= 0 {
                        self.opt_string(name_id as u32)?
                    } else {
                        None
                    };
                    let ty = if type_id >= 0 {
                        self.opt_type_descriptor(type_id as u32)?
                    } else {
                        None
                    };
                    let sign = if sign_id >= 0 {
                        self.opt_string(sign_id as u32)?
                    } else {
                        None
                    };
                    let v = ActiveVar {
                        reg,
                        name: name.unwrap_or_default(),
                        ty: ty.unwrap_or_default(),
                        signature: sign,
                        start: 0,
                        end_set: false,
                    };
                    start_var(&mut locals, &mut result, v, addr as i32);
                }
                DBG_RESTART_LOCAL => {
                    let reg = r.uleb128()? as usize;
                    if let Some(prev) = locals.get(reg).and_then(|s| s.clone()) {
                        if let Some(slot) = locals.get_mut(reg) {
                            if let Some(old) = slot.take() {
                                end_var(&mut result, old, addr as i32);
                            }
                        }
                        let v = ActiveVar {
                            reg: prev.reg,
                            name: prev.name,
                            ty: prev.ty,
                            signature: prev.signature,
                            start: addr as i32,
                            end_set: false,
                        };
                        if reg < locals.len() {
                            locals[reg] = Some(v);
                        }
                    }
                }
                DBG_END_LOCAL => {
                    let reg = r.uleb128()? as usize;
                    if let Some(slot) = locals.get_mut(reg) {
                        if let Some(v) = slot.take() {
                            end_var(&mut result, v, addr as i32);
                        }
                    }
                }
                DBG_SET_PROLOGUE_END | DBG_SET_EPILOGUE_BEGIN => {}
                DBG_SET_FILE => {
                    let idx = r.uleb128p1()?;
                    if idx >= 0 {
                        let _ = self.opt_string(idx as u32)?;
                    }
                }
                _ => {
                    let adjusted = i64::from(op) - DBG_FIRST_SPECIAL;
                    let addr_inc = (adjusted / DBG_LINE_RANGE) as u32;
                    addr = addr.saturating_add(addr_inc).min(code_last);
                    line += DBG_LINE_BASE + adjusted % DBG_LINE_RANGE;
                    out.lines.push((addr, line.max(0) as u32));
                }
            }
        }
        for slot in locals.into_iter().flatten() {
            end_var(&mut result, slot, code_last as i32);
        }
        out.locals = result;
        Ok(out)
    }
}

fn end_var(result: &mut Vec<LocalVar>, mut v: ActiveVar, addr: i32) {
    if v.end_set {
        return;
    }
    v.end_set = true;
    result.push(LocalVar {
        reg: v.reg,
        name: v.name,
        ty: v.ty,
        signature: v.signature,
        start: v.start,
        end: addr,
    });
}
