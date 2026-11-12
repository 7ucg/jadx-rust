import java.util.ArrayList;
import java.util.List;
import java.util.stream.Collectors;

import jadx.core.dex.attributes.AFlag;
import jadx.core.dex.attributes.AType;
import jadx.core.dex.attributes.AttrNode;
import jadx.core.dex.attributes.nodes.LoopInfo;
import jadx.core.dex.attributes.nodes.LoopLabelAttr;
import jadx.core.dex.attributes.AttrList;
import jadx.core.dex.instructions.IfNode;
import jadx.core.dex.instructions.SwitchInsn;
import jadx.core.dex.nodes.BlockNode;
import jadx.core.dex.nodes.IContainer;
import jadx.core.dex.nodes.IRegion;
import jadx.core.dex.nodes.InsnContainer;
import jadx.core.dex.nodes.InsnNode;
import jadx.core.dex.nodes.MethodNode;
import jadx.core.dex.regions.Region;
import jadx.core.dex.regions.SwitchRegion;
import jadx.core.dex.regions.SynchronizedRegion;
import jadx.core.dex.regions.TryCatchRegion;
import jadx.core.dex.regions.conditions.IfCondition;
import jadx.core.dex.regions.conditions.IfRegion;
import jadx.core.dex.regions.loops.ForEachLoop;
import jadx.core.dex.regions.loops.ForLoop;
import jadx.core.dex.regions.loops.LoopRegion;
import jadx.core.dex.trycatch.CatchAttr;
import jadx.core.dex.trycatch.ExcHandlerAttr;
import jadx.core.dex.trycatch.ExceptionHandler;

/**
 * Oracle helper: prints the region tree of a method in the format of `jadx-rs insn-regions`
 * (see crates/jadx-core/src/region_dump.rs). Every line starts with "R ".
 */
public class RegionDump {
	static final AFlag[] INSN_FLAGS = { AFlag.TRY_ENTER, AFlag.TRY_LEAVE, AFlag.SYNTHETIC, AFlag.DONT_GENERATE, AFlag.HIDDEN,
			AFlag.REMOVE, AFlag.WRAPPED, AFlag.FALL_THROUGH, AFlag.DONT_INLINE, AFlag.EXPLICIT_CAST, AFlag.SOFT_CAST,
			AFlag.FINALLY_INSNS, AFlag.FORCE_ASSIGN_INLINE };
	static final AFlag[] BLOCK_FLAGS = { AFlag.MTH_ENTER_BLOCK, AFlag.MTH_EXIT_BLOCK, AFlag.SYNTHETIC, AFlag.RETURN, AFlag.REMOVE,
			AFlag.LOOP_START, AFlag.LOOP_END, AFlag.EXC_TOP_SPLITTER, AFlag.EXC_BOTTOM_SPLITTER, AFlag.ADDED_TO_REGION,
			AFlag.DONT_GENERATE, AFlag.FALL_THROUGH, AFlag.ORIG_RETURN, AFlag.IGNORE_THROW_SPLIT, AFlag.TRY_ENTER, AFlag.TRY_LEAVE,
			AFlag.FINALLY_INSNS, AFlag.DUPLICATED, AFlag.DONT_INVERT, AFlag.ALLOW_MULTIPLE_INSNS_LOOP_COND, AFlag.ELSE_IF_CHAIN };

	static String arg(jadx.core.dex.instructions.args.InsnArg a) {
		return InsnNodeDump.arg(a);
	}

	static String cond(IfCondition c) {
		switch (c.getMode()) {
			case COMPARE:
				return arg(c.getCompare().getA()) + " " + c.getCompare().getOp().getSymbol() + " " + arg(c.getCompare().getB());
			case TERNARY:
				return cond(c.first()) + " ? " + cond(c.second()) + " : " + cond(c.third());
			case NOT:
				return "!(" + cond(c.first()) + ")";
			default:
				String op = c.getMode() == IfCondition.Mode.OR ? " || " : " && ";
				return "(" + c.getArgs().stream().map(RegionDump::cond).collect(Collectors.joining(op)) + ")";
		}
	}

	static String optCond(IfCondition c) {
		return c == null ? "-" : cond(c);
	}

	static String cids(List<BlockNode> list) {
		return list.stream().map(b -> Integer.toString(b.getCId())).collect(Collectors.joining(","));
	}

	static String opt(BlockNode b) {
		return b == null ? "-" : Integer.toString(b.getCId());
	}

	static void indent(StringBuilder sb, int ind) {
		sb.append("R ");
		for (int i = 0; i < ind; i++) {
			sb.append("  ");
		}
	}

	static String insnLine(InsnNode insn) {
		StringBuilder sb = new StringBuilder();
		sb.append(InsnNodeDump.line(insn));
		sb.append(" fl=[").append(BlockDump.flags(insn, INSN_FLAGS)).append(']');
		CatchAttr c = insn.get(AType.EXC_CATCH);
		if (c != null) {
			sb.append(" catch=[").append(c.getHandlers().stream().map(h -> Integer.toString(h.getHandlerOffset()))
					.collect(Collectors.joining(","))).append(']');
		}
		ExcHandlerAttr ih = insn.get(AType.EXC_HANDLER);
		if (ih != null) {
			sb.append(" exh=").append(ih.getHandler().getHandlerOffset());
		}
		if (insn instanceof IfNode) {
			IfNode f = (IfNode) insn;
			sb.append(" then=").append(f.getThenBlock() == null ? "-" : f.getThenBlock().getCId()).append(" else=")
					.append(f.getElseBlock() == null ? "-" : f.getElseBlock().getCId());
		}
		if (insn instanceof SwitchInsn) {
			SwitchInsn s = (SwitchInsn) insn;
			BlockNode[] tb = s.getTargetBlocks();
			sb.append(" tblocks=").append(tb == null ? "-" : cids(java.util.Arrays.asList(tb))).append(" defblock=")
					.append(s.getDefTargetBlock() == null ? "-" : s.getDefTargetBlock().getCId());
		}
		if (insn instanceof jadx.core.dex.instructions.mods.TernaryInsn) {
			sb.append(" cond=").append(cond(((jadx.core.dex.instructions.mods.TernaryInsn) insn).getCondition()));
		}
		AttrList<LoopInfo> loops = insn.get(AType.LOOP);
		if (loops != null) {
			sb.append(" loops=[").append(loops.getList().stream().map(l -> Integer.toString(l.getId())).collect(Collectors.joining(",")))
					.append(']');
		}
		LoopLabelAttr label = insn.get(AType.LOOP_LABEL);
		if (label != null) {
			sb.append(" label=").append(label.getLoop().getId());
		}
		return sb.toString();
	}

	static String excAttr(AttrNode n) {
		ExcHandlerAttr eh = n.get(AType.EXC_HANDLER);
		return eh == null ? "" : " exh=" + eh.getHandler().getHandlerOffset();
	}

	static void container(StringBuilder sb, IContainer c, int ind) {
		if (c instanceof BlockNode) {
			BlockNode b = (BlockNode) c;
			indent(sb, ind);
			sb.append("B ").append(b.getCId()).append(" flags=[").append(BlockDump.flags(b, BLOCK_FLAGS)).append(']');
			LoopLabelAttr label = b.get(AType.LOOP_LABEL);
			if (label != null) {
				sb.append(" label=").append(label.getLoop().getId());
			}
			sb.append('\n');
			for (InsnNode insn : b.getInstructions()) {
				indent(sb, ind + 1);
				sb.append(insnLine(insn)).append('\n');
			}
			return;
		}
		if (c instanceof InsnContainer) {
			InsnContainer ic = (InsnContainer) c;
			indent(sb, ind);
			sb.append("INSNS flags=[").append(BlockDump.flags(ic, BLOCK_FLAGS)).append("]\n");
			for (InsnNode insn : ic.getInstructions()) {
				indent(sb, ind + 1);
				sb.append(insnLine(insn)).append('\n');
			}
			return;
		}
		AttrNode an = (AttrNode) c;
		String flags = BlockDump.flags(an, BLOCK_FLAGS);
		String exh = excAttr(an);
		indent(sb, ind);
		if (c instanceof IfRegion) {
			IfRegion r = (IfRegion) c;
			sb.append("IF cond=").append(optCond(r.getCondition())).append(" cbs=[").append(cids(r.getConditionBlocks()))
					.append("] flags=[").append(flags).append(']').append(exh).append('\n');
			branch(sb, "THEN", r.getThenRegion(), ind + 1);
			branch(sb, "ELSE", r.getElseRegion(), ind + 1);
		} else if (c instanceof LoopRegion) {
			LoopRegion r = (LoopRegion) c;
			BlockNode pre = null;
			try {
				java.lang.reflect.Field f = LoopRegion.class.getDeclaredField("preCondition");
				f.setAccessible(true);
				pre = (BlockNode) f.get(r);
			} catch (ReflectiveOperationException e) {
				throw new RuntimeException(e);
			}
			String type = "-";
			if (r.getType() instanceof ForLoop) {
				ForLoop fl = (ForLoop) r.getType();
				type = "for init=[" + insnLine(fl.getInitInsn()) + "] incr=[" + insnLine(fl.getIncrInsn()) + "]";
			} else if (r.getType() instanceof ForEachLoop) {
				ForEachLoop fe = (ForEachLoop) r.getType();
				type = "foreach var=[" + insnLine(fe.getVarArg().getParentInsn()) + "] iter=["
						+ insnLine(fe.getIterableArg().getParentInsn()) + "]";
			}
			sb.append("LOOP id=").append(r.getInfo().getId()).append(" header=").append(opt(r.getHeader())).append(" end=")
					.append(r.isConditionAtEnd() ? 1 : 0).append(" pre=").append(opt(pre)).append(" cond=")
					.append(optCond(r.getCondition())).append(" cbs=[").append(cids(r.getConditionBlocks())).append("] type=")
					.append(type).append(" flags=[").append(flags).append(']').append(exh).append('\n');
			branch(sb, "BODY", r.getBody(), ind + 1);
		} else if (c instanceof SwitchRegion) {
			SwitchRegion r = (SwitchRegion) c;
			sb.append("SWITCH header=").append(r.getHeader().getCId()).append(" flags=[").append(flags).append(']').append(exh)
					.append('\n');
			// keys replaced by constant fields (ModVisitor) are printed as their int values
			java.util.IdentityHashMap<Object, Integer> keyValues = new java.util.IdentityHashMap<>();
			InsnNode swLast = r.getHeader().getInstructions().isEmpty() ? null
					: r.getHeader().getInstructions().get(r.getHeader().getInstructions().size() - 1);
			if (swLast instanceof SwitchInsn) {
				SwitchInsn si = (SwitchInsn) swLast;
				int[] ks = si.getKeys();
				for (int i = 0; i < ks.length; i++) {
					keyValues.put(si.getKey(i), ks[i]);
				}
			}
			for (SwitchRegion.CaseInfo ci : r.getCases()) {
				indent(sb, ind + 1);
				sb.append("CASE [").append(ci.getKeys().stream()
						.map(k -> k == SwitchRegion.DEFAULT_CASE_KEY ? "default"
								: (k instanceof Integer || k instanceof String || !keyValues.containsKey(k) ? k.toString()
										: Integer.toString(keyValues.get(k))))
						.collect(Collectors.joining(","))).append("]\n");
				container(sb, ci.getContainer(), ind + 2);
			}
		} else if (c instanceof SynchronizedRegion) {
			SynchronizedRegion r = (SynchronizedRegion) c;
			sb.append("SYNC enter=").append(Integer.toHexString(r.getEnterInsn().getOffset())).append(" exits=[")
					.append(r.getExitInsns().stream().map(i -> Integer.toHexString(i.getOffset())).collect(Collectors.joining(",")))
					.append("] flags=[").append(flags).append(']').append(exh).append('\n');
			for (IContainer sub : r.getSubBlocks()) {
				container(sb, sub, ind + 1);
			}
		} else if (c instanceof TryCatchRegion) {
			TryCatchRegion r = (TryCatchRegion) c;
			sb.append("TRY id=").append(r.getTryCatchBlock() == null ? "-" : Integer.toString(r.getTryCatchBlock().id()))
					.append(" flags=[").append(flags).append(']').append(exh).append('\n');
			branch(sb, "TRYBODY", r.getTryRegion(), ind + 1);
			for (java.util.Map.Entry<ExceptionHandler, IContainer> e : r.getCatchRegions().entrySet()) {
				branch(sb, "CATCH " + e.getKey().getHandlerOffset(), e.getValue(), ind + 1);
			}
			if (r.getFinallyRegion() != null) {
				branch(sb, "FINALLY", r.getFinallyRegion(), ind + 1);
			}
		} else if (c instanceof Region) {
			sb.append("REGION flags=[").append(flags).append(']').append(exh).append('\n');
			for (IContainer sub : ((IRegion) c).getSubBlocks()) {
				container(sb, sub, ind + 1);
			}
		} else {
			throw new RuntimeException("Unknown container: " + c.getClass());
		}
	}

	static void branch(StringBuilder sb, String name, IContainer c, int ind) {
		indent(sb, ind);
		if (c == null) {
			sb.append(name).append(" -\n");
		} else {
			sb.append(name).append('\n');
			container(sb, c, ind + 1);
		}
	}

	public static void dump(StringBuilder sb, MethodNode mth) {
		sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append('\n');
		if (mth.getRegion() != null) {
			container(sb, mth.getRegion(), 0);
		}
		for (ExceptionHandler h : mth.getExceptionHandlers()) {
			IContainer hr = h.getHandlerRegion();
			indent(sb, 0);
			sb.append("HANDLER ").append(h.getHandlerOffset()).append(hr == null ? " none" : "").append('\n');
			if (hr != null) {
				container(sb, hr, 1);
			}
		}
	}
}
