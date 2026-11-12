import java.util.List;
import java.util.stream.Collectors;

import jadx.core.dex.attributes.AFlag;
import jadx.core.dex.attributes.AType;
import jadx.core.dex.attributes.nodes.FieldReplaceAttr;
import jadx.core.dex.attributes.nodes.SkipMethodArgsAttr;
import jadx.core.dex.instructions.args.RegisterArg;
import jadx.core.dex.nodes.ClassNode;
import jadx.core.dex.nodes.FieldNode;
import jadx.core.dex.nodes.MethodNode;

/**
 * Oracle helper: class level state after the class passes (see crates/jadx-core/src/class_state_dump.rs):
 * access flags, hidden members and replacements of every class, field and method.
 */
public class ClassStateDump {
	public static void dump(StringBuilder sb, ClassNode cls) {
		sb.append("M CLASS ").append(cls.getClassInfo().getRawName()).append('\n');
		if (System.getenv("DUMP_CLS_ERR") != null) {
			for (jadx.core.dex.attributes.nodes.JadxError e : cls.getAll(AType.JADX_ERROR)) {
				System.err.println("CLSERR " + cls.getClassInfo().getRawName() + " " + e.getError() + " " + e.getCause());
			}
			jadx.core.dex.attributes.nodes.JadxCommentsAttr c = cls.get(AType.JADX_COMMENTS);
			if (c != null) {
				System.err.println("CLSCOMMENT " + cls.getClassInfo().getRawName() + " " + c.getComments());
			}
		}
		sb.append("R C acc=").append(Integer.toHexString(cls.getAccessFlags().rawValue()))
				.append(" dont=").append(cls.contains(AFlag.DONT_GENERATE) ? 1 : 0)
				.append(extraClassAttrs(cls)).append('\n');
		for (FieldNode f : cls.getFields()) {
			sb.append("R F ").append(f.getFieldInfo().getName()).append(':').append(f.getFieldInfo().getType())
					.append(" acc=").append(Integer.toHexString(f.getAccessFlags().rawValue()))
					.append(" dont=").append(f.contains(AFlag.DONT_GENERATE) ? 1 : 0)
					.append(" repl=").append(replace(cls, f))
					.append(extraFieldAttrs(f)).append('\n');
		}
		for (MethodNode m : cls.getMethods()) {
			SkipMethodArgsAttr skip = m.get(AType.SKIP_MTH_ARGS);
			StringBuilder skipStr = new StringBuilder();
			if (skip != null) {
				for (int i = 0; i < m.getMethodInfo().getArgsCount(); i++) {
					if (skip.isSkip(i)) {
						skipStr.append(i).append(',');
					}
				}
			}
			sb.append("R M ").append(m.getMethodInfo().getShortId())
					.append(" acc=").append(Integer.toHexString(m.getAccessFlags().rawValue()))
					.append(" dont=").append(m.contains(AFlag.DONT_GENERATE) ? 1 : 0)
					.append(" noskip=").append(m.contains(AFlag.NO_SKIP_ARGS) ? 1 : 0)
					.append(" skip=[").append(skipStr).append(']')
					.append(" skipfirst=").append(m.contains(AFlag.SKIP_FIRST_ARG) ? 1 : 0)
					.append(" mrepl=").append(m.get(AType.METHOD_REPLACE) == null ? "-" : m.get(AType.METHOD_REPLACE).getReplaceMth().getMethodInfo().getRawFullId()).append(extraMethodAttrs(m)).append('\n');
		}
	}

	private static String replace(ClassNode cls, FieldNode f) {
		FieldReplaceAttr r = f.get(AType.FIELD_REPLACE);
		if (r == null) {
			return "-";
		}
		if (r.getReplaceType() == FieldReplaceAttr.ReplaceWith.CLASS_INSTANCE) {
			return "cls:" + r.getClsRef().getRawName();
		}
		for (MethodNode m : cls.getMethods()) {
			List<RegisterArg> args = m.getArgRegs();
			for (int i = 0; i < args.size(); i++) {
				if (args.get(i) == r.getVarRef()) {
					return "var:" + m.getMethodInfo().getShortId() + ":" + i;
				}
			}
		}
		return "var:?";
	}

	private static String extraClassAttrs(ClassNode cls) {
		jadx.core.dex.attributes.nodes.EnumClassAttr e = cls.get(AType.ENUM_CLASS);
		StringBuilder sb = new StringBuilder();
		sb.append(" rmsuper=").append(cls.contains(AFlag.REMOVE_SUPER_CLASS) ? 1 : 0);
		if (e != null) {
			sb.append(" enum=[");
			for (jadx.core.dex.attributes.nodes.EnumClassAttr.EnumField f : e.getFields()) {
				sb.append(f.getField().getName()).append('|').append(f.getNameStr()).append('|')
						.append(f.getCls() == null ? "-" : f.getCls().getClassInfo().getRawName()).append('|')
						.append(RegionDump.insnLine(f.getConstrInsn())).append(';');
			}
			sb.append(']');
		}
		return sb.toString();
	}

	private static String extraFieldAttrs(FieldNode f) {
		jadx.core.dex.attributes.FieldInitInsnAttr init = f.get(AType.FIELD_INIT_INSN);
		if (init == null) {
			return "";
		}
		return " init=" + init.getInsnMth().getMethodInfo().getShortId() + "|" + RegionDump.insnLine(init.getInsn());
	}

	private static String extraMethodAttrs(MethodNode m) {
		return "";
	}
}
