import java.io.File;
import java.util.List;

import jadx.api.JadxArgs;
import jadx.api.JadxDecompiler;
import jadx.core.dex.attributes.AType;
import jadx.core.dex.attributes.nodes.JumpInfo;
import jadx.core.dex.info.FieldInfo;
import jadx.core.dex.instructions.ArithNode;
import jadx.core.dex.instructions.ConstClassNode;
import jadx.core.dex.instructions.mods.ConstructorInsn;
import jadx.core.dex.instructions.ConstStringNode;
import jadx.core.dex.instructions.FillArrayData;
import jadx.core.dex.instructions.FillArrayInsn;
import jadx.core.dex.instructions.FilledNewArrayNode;
import jadx.core.dex.instructions.GotoNode;
import jadx.core.dex.instructions.IfNode;
import jadx.core.dex.instructions.IndexInsnNode;
import jadx.core.dex.instructions.InvokeNode;
import jadx.core.dex.instructions.NewArrayNode;
import jadx.core.dex.instructions.SwitchData;
import jadx.core.dex.instructions.SwitchInsn;
import jadx.core.dex.instructions.args.InsnArg;
import jadx.core.dex.instructions.args.LiteralArg;
import jadx.core.dex.instructions.args.InsnWrapArg;
import jadx.core.dex.instructions.args.RegisterArg;
import jadx.core.dex.nodes.ClassNode;
import jadx.core.dex.nodes.InsnNode;
import jadx.core.dex.nodes.MethodNode;
import jadx.core.dex.nodes.RootNode;
import jadx.core.dex.visitors.ProcessInstructionsVisitor;
import jadx.core.dex.attributes.AFlag;

/**
 * Oracle helper: runs the reference decoder (InsnDecoder + ProcessInstructionsVisitor) and dumps
 * every instruction node in the format of `jadx-rs insn-nodes` (see crates/jadx-core/src/dump.rs).
 */
public class InsnNodeDump {
	public static void main(String[] args) throws Exception {
		JadxArgs jargs = new JadxArgs();
		jargs.setInputFile(new File(args[0]));
		jargs.setSkipResources(true);
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
						new ProcessInstructionsVisitor().visit(mth);
					} catch (Throwable e) {
						sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append(" LOAD_ERROR\n");
						System.out.print(sb);
						sb.setLength(0);
						continue;
					}
					sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append('\n');
					for (InsnNode insn : mth.getInstructions()) {
						if (insn != null) {
							dump(sb, insn);
						}
					}
					System.out.print(sb);
					sb.setLength(0);
					mth.unload();
				}
			}
		}
	}

	/** Print SSA versions (`r1:int/v0`) when set. */
	static boolean SSA = false;

	private static String reg(RegisterArg r) {
		String s = "r" + r.getRegNum() + ":" + r.getInitType() + (r.contains(AFlag.IMMUTABLE_TYPE) ? "!" : "");
		if (SSA && r.getSVar() != null) {
			s += "/v" + r.getSVar().getVersion();
		}
		return s;
	}

	private static String arg(InsnArg a) {
		if (a instanceof RegisterArg) {
			return reg((RegisterArg) a);
		}
		if (a instanceof InsnWrapArg) {
			InsnWrapArg w = (InsnWrapArg) a;
			return "(wrap " + w.getType() + ":" + line(w.getWrapInsn()).substring(2) + ")";
		}
		if (a instanceof LiteralArg) {
			LiteralArg l = (LiteralArg) a;
			return "#" + l.getLiteral() + ":" + l.getType();
		}
		return "?" + a;
	}

	private static FillArrayData fillData(FillArrayInsn f) {
		try {
			java.lang.reflect.Field fld = FillArrayInsn.class.getDeclaredField("arrayData");
			fld.setAccessible(true);
			return (FillArrayData) fld.get(f);
		} catch (ReflectiveOperationException e) {
			throw new RuntimeException(e);
		}
	}

	private static String fill(FillArrayData d) {
		return d.getSize() + ":" + d.getElementType() + ":" + d.dataToString();
	}

	static String line(InsnNode n) {
		StringBuilder sb = new StringBuilder();
		dump(sb, n);
		sb.setLength(sb.length() - 1);
		return sb.toString();
	}

	static void dump(StringBuilder sb, InsnNode n) {
		sb.append("I ").append(Integer.toHexString(n.getOffset())).append(' ').append(n.getType());
		sb.append(" res=").append(n.getResult() == null ? "-" : reg(n.getResult()));
		sb.append(" args=[");
		for (int i = 0; i < n.getArgsCount(); i++) {
			sb.append(i == 0 ? "" : ",").append(arg(n.getArg(i)));
		}
		sb.append(']');
		if (n instanceof IfNode) {
			IfNode f = (IfNode) n;
			sb.append(" if=").append(f.getOp()).append(" target=").append(f.getTarget());
		} else if (n instanceof GotoNode) {
			sb.append(" goto=").append(((GotoNode) n).getTarget());
		} else if (n instanceof SwitchInsn) {
			SwitchInsn s = (SwitchInsn) n;
			sb.append(" switch packed=").append(s.isPacked()).append(" data=").append(s.getDataTarget()).append(" def=")
					.append(s.getDefaultCaseOffset());
			if (s.needData()) {
				sb.append(" nodata");
			} else {
				sb.append(" keys=").append(java.util.Arrays.toString(s.getKeys())).append(" targets=")
						.append(java.util.Arrays.toString(s.getTargets()));
			}
		} else if (n instanceof SwitchData) {
			SwitchData d = (SwitchData) n;
			sb.append(" swdata keys=").append(java.util.Arrays.toString(d.getKeys())).append(" targets=")
					.append(java.util.Arrays.toString(d.getTargets()));
		} else if (n instanceof ConstStringNode) {
			String str = ((ConstStringNode) n).getString();
			sb.append(" str=[");
			for (int i = 0; i < str.length(); i++) {
				sb.append(i == 0 ? "" : ",").append(Integer.toHexString(str.charAt(i)));
			}
			sb.append(']');
		} else if (n instanceof ConstClassNode) {
			sb.append(" cls=").append(((ConstClassNode) n).getClsType());
		} else if (n instanceof IndexInsnNode) {
			Object idx = ((IndexInsnNode) n).getIndex();
			sb.append(" idx=").append(idx instanceof FieldInfo ? idx.toString() : String.valueOf(idx));
		} else if (n instanceof InvokeNode) {
			InvokeNode in = (InvokeNode) n;
			sb.append(" invoke=").append(in.getInvokeType()).append(' ').append(in.getCallMth());
		} else if (n instanceof ConstructorInsn) {
			ConstructorInsn c = (ConstructorInsn) n;
			sb.append(" ctor ").append(c.getCallMth()).append(' ').append(c.getCallType());
		} else if (n instanceof ArithNode) {
			sb.append(" op=").append(((ArithNode) n).getOp());
		} else if (n instanceof NewArrayNode) {
			sb.append(" arr=").append(((NewArrayNode) n).getArrayType());
		} else if (n instanceof FilledNewArrayNode) {
			sb.append(" elem=").append(((FilledNewArrayNode) n).getElemType());
		} else if (n instanceof FillArrayInsn) {
			FillArrayInsn f = (FillArrayInsn) n;
			sb.append(" fill target=").append(f.getTarget());
			FillArrayData fd = fillData(f);
			if (fd == null) {
				sb.append(" nodata");
			} else {
				sb.append(" data=").append(fill(fd));
			}
		} else if (n instanceof FillArrayData) {
			sb.append(" fdata=").append(fill((FillArrayData) n));
		}
		List<JumpInfo> jumps = n.getAll(AType.JUMP);
		if (!jumps.isEmpty()) {
			sb.append(" jumps=[");
			for (int i = 0; i < jumps.size(); i++) {
				sb.append(i == 0 ? "" : ",").append(jumps.get(i).getSrc()).append('>').append(jumps.get(i).getDest());
			}
			sb.append(']');
		}
		if (n.contains(AType.JADX_ERROR)) {
			sb.append(" ERROR");
		}
		sb.append('\n');
	}
}
