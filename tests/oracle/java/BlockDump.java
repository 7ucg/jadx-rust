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
import jadx.core.dex.visitors.AttachTryCatchVisitor;
import jadx.core.dex.visitors.ProcessInstructionsVisitor;
import jadx.core.dex.visitors.blocks.BlockSplitter;

/**
 * Oracle helper: runs AttachTryCatch + ProcessInstructions + BlockSplitter of the reference and dumps
 * the block graph in the format of `jadx-rs insn-blocks` (see crates/jadx-core/src/dump.rs).
 */
public class BlockDump {
	static final AFlag[] INSN_FLAGS = { AFlag.TRY_ENTER, AFlag.TRY_LEAVE, AFlag.SYNTHETIC, AFlag.DONT_GENERATE, AFlag.HIDDEN,
			AFlag.REMOVE, AFlag.WRAPPED, AFlag.FALL_THROUGH };
	static final AFlag[] BLOCK_FLAGS = { AFlag.MTH_ENTER_BLOCK, AFlag.MTH_EXIT_BLOCK, AFlag.SYNTHETIC, AFlag.RETURN, AFlag.REMOVE,
			AFlag.LOOP_START, AFlag.LOOP_END, AFlag.EXC_TOP_SPLITTER, AFlag.EXC_BOTTOM_SPLITTER, AFlag.ADDED_TO_REGION,
			AFlag.DONT_GENERATE, AFlag.FALL_THROUGH };

	public static void main(String[] args) throws Exception {
		JadxArgs jargs = new JadxArgs();
		jargs.setInputFile(new File(args[0]));
		jargs.setSkipResources(true);
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
					} catch (Throwable e) {
						sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append(" LOAD_ERROR\n");
						System.out.print(sb);
						sb.setLength(0);
						continue;
					}
					dump(sb, mth);
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

	static void dump(StringBuilder sb, MethodNode mth) {
		sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append('\n');
		for (ExceptionHandler h : mth.getExceptionHandlers()) {
			sb.append("H ").append(h.getHandlerOffset()).append(" types=[")
					.append(h.getCatchTypes().stream().map(c -> c.getFullName()).collect(Collectors.joining(","))).append("]\n");
		}
		for (BlockNode b : mth.getBasicBlocks()) {
			sb.append("B ").append(b.getCId()).append(" pos=").append(b.getPos()).append(" off=").append(b.getStartOffset())
					.append(" flags=[").append(flags(b, BLOCK_FLAGS)).append("] preds=[").append(cids(b.getPredecessors()))
					.append("] succs=[").append(cids(b.getSuccessors())).append(']');
			var tmp = b.get(AType.TMP_EDGE);
			sb.append(" tmp=").append(tmp == null ? "-" : Integer.toString(tmp.getBlock().getCId()));
			ExcHandlerAttr eh = b.get(AType.EXC_HANDLER);
			sb.append(" exh=").append(eh == null ? "-" : Integer.toString(eh.getHandler().getHandlerOffset())).append('\n');
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
	}
}
