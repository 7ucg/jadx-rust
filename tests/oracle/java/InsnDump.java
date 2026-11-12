import java.nio.file.Path;
import java.util.List;

import jadx.api.plugins.input.data.IClassData;
import jadx.api.plugins.input.data.ICodeReader;
import jadx.api.plugins.input.data.IFieldData;
import jadx.api.plugins.input.data.IMethodData;
import jadx.api.plugins.input.data.ISeqConsumer;
import jadx.api.plugins.input.insns.InsnData;
import jadx.api.plugins.input.insns.custom.IArrayPayload;
import jadx.api.plugins.input.insns.custom.ISwitchPayload;
import jadx.plugins.input.dex.DexFileLoader;
import jadx.plugins.input.dex.DexInputOptions;
import jadx.plugins.input.dex.DexReader;
import jadx.plugins.input.dex.insns.DexInsnData;
import jadx.plugins.input.dex.insns.DexInsnFormat;

/**
 * Oracle helper: dumps every instruction of every method as seen by the reference input plugin.
 * Same format as `jadx-rs insn-dump`. Fields that the instruction format does not carry are not
 * printed (the reference reuses one mutable object, so they would hold stale values).
 */
public class InsnDump {
	public static void main(String[] args) throws Exception {
		DexInputOptions opts = new DexInputOptions();
		opts.registerOptions();
		DexFileLoader loader = new DexFileLoader(opts);
		List<DexReader> readers = loader.collectDexFiles(List.of(Path.of(args[0])));
		StringBuilder sb = new StringBuilder();
		for (DexReader reader : readers) {
			reader.visitClasses(cls -> {
				cls.visitFieldsAndMethods(new ISeqConsumer<IFieldData>() {
					public void init(int count) { }
					public void accept(IFieldData f) { }
				}, new ISeqConsumer<IMethodData>() {
					public void init(int count) { }
					public void accept(IMethodData m) {
						ICodeReader code = m.getCodeReader();
						if (code == null) {
							return;
						}
						var ref = (jadx.plugins.input.dex.sections.DexMethodRef) m.getMethodRef();
						ref.load();
						sb.append("M ").append(ref.getParentClassType()).append("->").append(ref.getName()).append('(')
								.append(String.join("", ref.getArgTypes())).append(')').append(ref.getReturnType()).append(" regs=").append(code.getRegistersCount())
								.append(" units=").append(code.getUnitsCount()).append('\n');
						code.visitInstructions(insn -> dump(sb, insn));
						System.out.print(sb);
						sb.setLength(0);
					}
				});
			});
		}
	}

	private static void dump(StringBuilder sb, InsnData insn) {
		insn.decode();
		DexInsnData d = (DexInsnData) insn;
		sb.append("I ").append(Integer.toHexString(insn.getOffset())).append(' ').append(insn.getOpcode());
		var info = d.getInsnInfo();
		if (info == null) {
			sb.append(" raw=").append(Integer.toHexString(insn.getRawOpcodeUnit())).append('\n');
			return;
		}
		DexInsnFormat f = info.getFormat();
		int rc = insn.getRegsCount();
		sb.append(" r=[");
		for (int i = 0; i < rc; i++) {
			sb.append(i == 0 ? "" : ",").append(insn.getReg(i));
		}
		sb.append(']');
		boolean hasLit = has(f, "FORMAT_11N", "FORMAT_21S", "FORMAT_21H", "FORMAT_22B", "FORMAT_22S", "FORMAT_22C", "FORMAT_31I", "FORMAT_51I");
		boolean hasTarget = has(f, "FORMAT_10T", "FORMAT_20T", "FORMAT_21T", "FORMAT_22T", "FORMAT_30T", "FORMAT_31T", "FORMAT_45CC", "FORMAT_4RCC");
		boolean hasIndex = has(f, "FORMAT_21C", "FORMAT_22C", "FORMAT_31C", "FORMAT_35C", "FORMAT_3RC", "FORMAT_45CC", "FORMAT_4RCC");
		sb.append(" l=").append(hasLit ? insn.getLiteral() : 0);
		sb.append(" t=").append(hasTarget ? insn.getTarget() : 0);
		sb.append(" i=").append(hasIndex ? insn.getIndex() : 0).append(" it=").append(insn.getIndexType());
		var p = insn.getPayload();
		if (p instanceof ISwitchPayload) {
			ISwitchPayload sp = (ISwitchPayload) p;
			sb.append(" switch keys=").append(java.util.Arrays.toString(sp.getKeys()))
					.append(" targets=").append(java.util.Arrays.toString(sp.getTargets()));
		} else if (p instanceof IArrayPayload) {
			IArrayPayload ap = (IArrayPayload) p;
			sb.append(" array size=").append(ap.getSize()).append(" elem=").append(ap.getElementSize()).append(" data=");
			Object data = ap.getData();
			if (data instanceof byte[]) {
				sb.append(java.util.Arrays.toString((byte[]) data));
			} else if (data instanceof short[]) {
				sb.append(java.util.Arrays.toString((short[]) data));
			} else if (data instanceof int[]) {
				sb.append(java.util.Arrays.toString((int[]) data));
			} else if (data instanceof long[]) {
				sb.append(java.util.Arrays.toString((long[]) data));
			}
		}
		sb.append('\n');
	}

	private static boolean has(DexInsnFormat f, String... names) {
		try {
			for (String n : names) {
				if (DexInsnFormat.class.getField(n).get(null) == f) {
					return true;
				}
			}
		} catch (ReflectiveOperationException e) {
			throw new RuntimeException(e);
		}
		return false;
	}
}
