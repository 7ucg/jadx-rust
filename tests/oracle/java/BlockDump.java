import java.io.File;
import java.util.List;
import java.util.stream.Collectors;

import jadx.api.JadxArgs;
import jadx.api.JadxDecompiler;
import jadx.core.dex.attributes.AFlag;
import jadx.core.dex.attributes.AType;
import jadx.core.dex.instructions.IfNode;
import jadx.core.dex.instructions.SwitchInsn;
import jadx.core.dex.nodes.BlockNode;
import jadx.core.dex.nodes.ClassNode;
import jadx.core.dex.nodes.InsnNode;
import jadx.core.dex.nodes.MethodNode;
import jadx.core.dex.nodes.RootNode;
import jadx.core.dex.trycatch.CatchAttr;
import jadx.core.dex.trycatch.ExcHandlerAttr;
import jadx.core.dex.trycatch.ExceptionHandler;
import jadx.core.dex.trycatch.TryCatchBlockAttr;
import jadx.core.dex.visitors.AttachTryCatchVisitor;
import jadx.core.dex.visitors.ProcessInstructionsVisitor;
import jadx.core.dex.visitors.blocks.BlockProcessor;
import jadx.core.dex.visitors.blocks.BlockFinisher;
import jadx.core.dex.visitors.ssa.SSATransform;
import jadx.core.dex.visitors.MoveInlineVisitor;
import jadx.core.dex.visitors.ConstructorVisitor;
import jadx.core.dex.visitors.InitCodeVariables;
import jadx.core.dex.instructions.args.CodeVar;
import jadx.core.dex.instructions.PhiInsn;
import jadx.core.dex.instructions.args.RegisterArg;
import jadx.core.dex.instructions.args.SSAVar;
import jadx.core.dex.attributes.nodes.PhiListAttr;
import java.util.IdentityHashMap;
import java.util.Map;
import jadx.core.dex.visitors.blocks.BlockSplitter;
import jadx.core.dex.attributes.nodes.LoopInfo;

/**
 * Oracle helper: runs AttachTryCatch + ProcessInstructions + BlockSplitter of the reference and dumps
 * the block graph in the format of `jadx-rs insn-blocks` (see crates/jadx-core/src/dump.rs).
 */
public class BlockDump {
	static final AFlag[] INSN_FLAGS = { AFlag.TRY_ENTER, AFlag.TRY_LEAVE, AFlag.SYNTHETIC, AFlag.DONT_GENERATE, AFlag.HIDDEN,
			AFlag.REMOVE, AFlag.WRAPPED, AFlag.FALL_THROUGH, AFlag.DONT_INLINE };
	static final AFlag[] BLOCK_FLAGS = { AFlag.MTH_ENTER_BLOCK, AFlag.MTH_EXIT_BLOCK, AFlag.SYNTHETIC, AFlag.RETURN, AFlag.REMOVE,
			AFlag.LOOP_START, AFlag.LOOP_END, AFlag.EXC_TOP_SPLITTER, AFlag.EXC_BOTTOM_SPLITTER, AFlag.ADDED_TO_REGION,
			AFlag.DONT_GENERATE, AFlag.FALL_THROUGH, AFlag.ORIG_RETURN, AFlag.IGNORE_THROW_SPLIT, AFlag.TRY_ENTER, AFlag.TRY_LEAVE };

	public static void main(String[] args) throws Exception {
		JadxArgs jargs = new JadxArgs();
		jargs.setInputFile(new File(args[0]));
		jargs.setSkipResources(true);
		if (args.length > 1 && Integer.parseInt(args[1]) >= 4) {
			// anonymous class inlining is not ported yet
			jargs.setInlineAnonymousClasses(false);
		}
		int stage = args.length > 1 ? Integer.parseInt(args[1]) : 0;
		StringBuilder sb = new StringBuilder();
		try (JadxDecompiler jadx = new JadxDecompiler(jargs)) {
			jadx.load();
			RootNode root = jadx.getRoot();
			for (ClassNode cls : root.getClasses()) {
				for (MethodNode mth : cls.getMethods()) {
					if (mth.isNoCode()) {
						continue;
					}
					try {
						mth.load();
						new AttachTryCatchVisitor().visit(mth);
						new ProcessInstructionsVisitor().visit(mth);
						new BlockSplitter().visit(mth);
						if (stage >= 1) {
							new BlockProcessor().visit(mth);
						}
						if (stage >= 2) {
							new BlockFinisher().visit(mth);
							new SSATransform().visit(mth);
							InsnNodeDump.SSA = true;
						}
						if (stage >= 3) {
							new MoveInlineVisitor().visit(mth);
						}
						if (stage >= 4) {
							new ConstructorVisitor().visit(mth);
						}
						if (stage >= 5) {
							new InitCodeVariables().visit(mth);
						}
					} catch (Throwable e) {
						sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append(" LOAD_ERROR\n");
						System.out.print(sb);
						sb.setLength(0);
						continue;
					}
					dump(sb, mth, stage >= 1, stage >= 2);
					System.out.print(sb);
					sb.setLength(0);
					mth.unload();
				}
			}
		}
	}

	static String flags(jadx.core.dex.attributes.AttrNode n, AFlag[] all) {
		return java.util.Arrays.stream(all).filter(n::contains).map(Enum::name).sorted().collect(Collectors.joining(","));
	}

	static String cids(List<BlockNode> list) {
		return list.stream().map(b -> Integer.toString(b.getCId())).collect(Collectors.joining(","));
	}

	static String bits(java.util.BitSet bs) {
		if (bs == null) {
			return "-";
		}
		return bs.stream().mapToObj(Integer::toString).collect(Collectors.joining(","));
	}

	static String cid(BlockNode b) {
		return b == null ? "-" : Integer.toString(b.getCId());
	}

	static Map<InsnNode, String> locations(MethodNode mth) {
		Map<InsnNode, String> map = new IdentityHashMap<>();
		for (BlockNode b : mth.getBasicBlocks()) {
			int i = 0;
			for (InsnNode insn : b.getInstructions()) {
				map.put(insn, "b" + b.getCId() + "i" + i++);
			}
			PhiListAttr pl = b.get(AType.PHI_LIST);
			if (pl != null) {
				for (PhiInsn phi : pl.getList()) {
					map.put(phi, "p" + b.getCId() + "r" + phi.getResult().getRegNum());
				}
			}
		}
		return map;
	}

	static String argLoc(MethodNode mth, Map<InsnNode, String> locs, RegisterArg arg) {
		InsnNode parent = arg.getParentInsn();
		if (parent == null) {
			if (arg == mth.getThisArg()) {
				return "this";
			}
			return "m" + mth.getArgRegs().indexOf(arg);
		}
		String l = locs.getOrDefault(parent, "gone");
		if (parent.getResult() == arg) {
			return l + ":res";
		}
		for (int k = 0; k < parent.getArgsCount(); k++) {
			if (parent.getArg(k) == arg) {
				return l + ":" + k;
			}
		}
		return l + ":?";
	}

	static void dumpSsa(StringBuilder sb, MethodNode mth) {
		Map<InsnNode, String> locs = locations(mth);
		Map<CodeVar, Integer> cvIds = new IdentityHashMap<>();
		for (SSAVar v : mth.getSVars()) {
			sb.append("V r").append(v.getRegNum()).append("/v").append(v.getVersion()).append(" assign=")
					.append(argLoc(mth, locs, v.getAssign())).append(" uses=[")
					.append(v.getUseList().stream().map(a -> argLoc(mth, locs, a)).collect(Collectors.joining(",")))
					.append("] phis=[");
			if (v.getUsedInPhi().isEmpty()) {
				sb.append('-');
			} else {
				sb.append(v.getUsedInPhi().stream().map(p -> locs.getOrDefault(p, "gone")).collect(Collectors.joining(",")));
			}
			sb.append(']');
			if (v.isCodeVarSet()) {
				CodeVar cv = v.getCodeVar();
				int n = cvIds.computeIfAbsent(cv, k -> cvIds.size());
				java.util.List<String> fl = new java.util.ArrayList<>();
				if (cv.isDeclared()) {
					fl.add("declared");
				}
				if (cv.isFinal()) {
					fl.add("final");
				}
				if (cv.isThis()) {
					fl.add("this");
				}
				sb.append(" cv=").append(n).append('[').append(String.join(",", fl)).append("] cvty=")
						.append(cv.getType() == null ? "-" : cv.getType().toString()).append(" cvname=")
						.append(cv.getName() == null ? "-" : cv.getName());
			}
			sb.append('\n');
		}
	}

	static void dump(StringBuilder sb, MethodNode mth, boolean analysis, boolean ssa) {
		sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append('\n');
		for (ExceptionHandler h : mth.getExceptionHandlers()) {
			sb.append("H ").append(h.getHandlerOffset()).append(" types=[")
					.append(h.getCatchTypes().stream().map(c -> c.getFullName()).collect(Collectors.joining(","))).append("]\n");
		}
		if (analysis) {
			List<TryCatchBlockAttr> tries = mth.getAll(AType.TRY_BLOCKS_LIST);
			if (tries != null) {
				for (TryCatchBlockAttr t : tries) {
					sb.append("T ").append(t.id()).append(" handlers=[")
							.append(t.getHandlers().stream().map(h -> Integer.toString(h.getHandlerOffset())).collect(Collectors.joining(",")))
							.append("] blocks=[").append(cids(t.getBlocks())).append("] outer=")
							.append(t.getOuterTryBlock() == null ? "-" : Integer.toString(t.getOuterTryBlock().id())).append(" inner=[")
							.append(t.getInnerTryBlocks().stream().map(i -> Integer.toString(i.id())).collect(Collectors.joining(",")))
							.append("] top=").append(t.getTopSplitter() == null ? "-" : Integer.toString(t.getTopSplitter().getCId())).append('\n');
				}
			}
			for (LoopInfo l : mth.getLoops()) {
				sb.append("L ").append(l.getId()).append(" start=").append(l.getStart().getCId()).append(" end=").append(l.getEnd().getCId())
						.append(" parent=").append(l.getParentLoop() == null ? "-" : Integer.toString(l.getParentLoop().getId()))
						.append(" blocks=[").append(l.getLoopBlocks().stream().map(b -> b.getCId()).sorted().map(Object::toString)
								.collect(Collectors.joining(","))).append("]\n");
			}
		}
		for (BlockNode b : mth.getBasicBlocks()) {
			sb.append("B ").append(b.getCId()).append(" pos=").append(b.getPos()).append(" off=").append(b.getStartOffset())
					.append(" flags=[").append(flags(b, BLOCK_FLAGS)).append("] preds=[").append(cids(b.getPredecessors()))
					.append("] succs=[").append(cids(b.getSuccessors())).append(']');
			var tmp = b.get(AType.TMP_EDGE);
			sb.append(" tmp=").append(tmp == null ? "-" : Integer.toString(tmp.getBlock().getCId()));
			ExcHandlerAttr eh = b.get(AType.EXC_HANDLER);
			sb.append(" exh=").append(eh == null ? "-" : Integer.toString(eh.getHandler().getHandlerOffset()));
			if (analysis) {
				sb.append(" doms=[").append(bits(b.getDoms())).append("] idom=").append(cid(b.getIDom())).append(" df=[")
						.append(bits(b.getDomFrontier())).append("] pdoms=[").append(bits(b.getPostDoms())).append("] ipdom=")
						.append(cid(b.getIPostDom())).append(" domon=[").append(cids(b.getDominatesOn())).append("] clean=[")
						.append(b.getCleanSuccessors() == null ? "-" : cids(b.getCleanSuccessors())).append("] loops=[")
						.append(b.getAll(AType.LOOP).stream().map(l -> l.getId() + ":" + l.getStart().getCId() + ">" + l.getEnd().getCId())
								.collect(Collectors.joining(",")))
						.append(']');
			}
			sb.append('\n');
			if (ssa) {
				PhiListAttr pl = b.get(AType.PHI_LIST);
				if (pl != null) {
					for (PhiInsn phi : pl.getList()) {
						sb.append("  P ").append(InsnNodeDump.line(phi).substring(2)).append(" binds=[");
						for (int i = 0; i < phi.getArgsCount(); i++) {
							sb.append(i == 0 ? "" : ",").append(phi.getBlockByArgIndex(i).getCId());
						}
						sb.append("]\n");
					}
				}
			}
			for (InsnNode insn : b.getInstructions()) {
				sb.append("  ").append(InsnNodeDump.line(insn));
				sb.append(" fl=[").append(flags(insn, INSN_FLAGS)).append(']');
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
				sb.append('\n');
			}
		}
		if (ssa) {
			dumpSsa(sb, mth);
		}
	}
}
