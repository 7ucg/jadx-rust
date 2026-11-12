import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collection;
import java.util.Comparator;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;
import java.util.stream.Collectors;

import jadx.api.plugins.input.data.IClassData;
import jadx.api.plugins.input.data.ICodeReader;
import jadx.api.plugins.input.data.IDebugInfo;
import jadx.api.plugins.input.data.IFieldData;
import jadx.api.plugins.input.data.IFieldRef;
import jadx.api.plugins.input.data.IMethodData;
import jadx.api.plugins.input.data.IMethodHandle;
import jadx.api.plugins.input.data.IMethodProto;
import jadx.api.plugins.input.data.IMethodRef;
import jadx.api.plugins.input.data.ICallSite;
import jadx.api.plugins.input.data.ILocalVar;
import jadx.api.plugins.input.data.ISeqConsumer;
import jadx.api.plugins.input.data.ITry;
import jadx.api.plugins.input.data.annotations.EncodedValue;
import jadx.api.plugins.input.data.annotations.IAnnotation;
import jadx.api.plugins.input.data.attributes.IJadxAttribute;
import jadx.api.plugins.input.data.attributes.types.AnnotationDefaultAttr;
import jadx.api.plugins.input.data.attributes.types.AnnotationMethodParamsAttr;
import jadx.api.plugins.input.data.attributes.types.AnnotationsAttr;
import jadx.api.plugins.input.data.attributes.types.ExceptionsAttr;
import jadx.api.plugins.input.data.attributes.types.InnerClassesAttr;
import jadx.api.plugins.input.data.attributes.types.InnerClsInfo;
import jadx.api.plugins.input.data.attributes.types.MethodParametersAttr;
import jadx.api.plugins.input.data.attributes.types.SignatureAttr;
import jadx.api.plugins.input.data.attributes.types.SourceFileAttr;
import jadx.api.plugins.input.insns.InsnData;
import jadx.api.plugins.input.insns.Opcode;
import jadx.api.plugins.input.insns.custom.ISwitchPayload;
import jadx.plugins.input.java.JavaClassReader;
import jadx.plugins.input.java.JavaInputLoader;
import jadx.plugins.input.java.data.JavaMethodRef;
import jadx.plugins.input.java.data.code.JavaInsnData;

/**
 * Oracle helper: dumps the class data (`classes`) or the decoded instruction stream (`insns`) of
 * every class of a .class/.jar input as seen by the reference java input plugin. Same format as
 * `jadx-rs java-classes` / `jadx-rs java-insns`.
 *
 * The reference reuses one mutable instruction object, so operands that an instruction does not
 * set would hold stale values. They are reset to sentinels before decoding and printed as
 * "not set" (trailing registers are dropped, literal/target/index are 0).
 */
public class JavaInputDump {
	private static final int SENT_REG = -123456789;
	private static final int SENT_INT = Integer.MIN_VALUE + 17;
	private static final long SENT_LIT = 0x5EAD_BEEF_1234_5678L;

	public static void main(String[] args) throws Exception {
		String mode = args[0];
		@SuppressWarnings("deprecation")
		JavaInputLoader loader = new JavaInputLoader();
		List<JavaClassReader> readers = loader.collectFiles(List.of(Path.of(args[1])));
		for (JavaClassReader reader : readers) {
			IClassData cls;
			try {
				cls = reader.loadClassData();
			} catch (Exception e) {
				continue;
			}
			StringBuilder sb = new StringBuilder();
			if (mode.equals("classes")) {
				dumpClass(sb, reader, cls);
			} else {
				dumpInsns(sb, cls);
			}
			System.out.print(sb);
		}
	}

	// ---- class data ----

	private static void dumpClass(StringBuilder sb, JavaClassReader reader, IClassData cls) {
		String type = cls.getType();
		String sup = cls.getSuperType();
		sb.append("C ").append(reader.getFileName()).append('\t').append(type).append('\t')
				.append("0x").append(Integer.toHexString(cls.getAccessFlags())).append('\t')
				.append(sup == null ? "null" : sup).append('\t')
				.append(String.join(",", cls.getInterfacesTypes())).append('\n');
		String sig = null;
		String srcFile = null;
		InnerClassesAttr inner = null;
		List<IAnnotation> anns = new ArrayList<>();
		for (IJadxAttribute attr : cls.getAttributes()) {
			if (attr instanceof SignatureAttr) {
				sig = ((SignatureAttr) attr).getSignature();
			} else if (attr instanceof SourceFileAttr) {
				srcFile = ((SourceFileAttr) attr).getFileName();
			} else if (attr instanceof InnerClassesAttr) {
				inner = (InnerClassesAttr) attr;
			} else if (attr instanceof AnnotationsAttr) {
				anns.addAll(((AnnotationsAttr) attr).getAll());
			}
		}
		sb.append("CS sig=").append(sig).append(" src=").append(srcFile).append('\n');
		if (inner != null) {
			TreeMap<String, InnerClsInfo> sorted = new TreeMap<>(inner.getMap());
			for (InnerClsInfo i : sorted.values()) {
				sb.append("CI ").append(innerInfo(i)).append('\n');
			}
			InnerClsInfo own = inner.getMap().get(type);
			sb.append("CN ").append(own == null ? "none" : innerInfo(own)).append('\n');
		} else {
			sb.append("CN none\n");
		}
		annotations(sb, "CA", anns);
		sb.append("CR ").append(refs(anns)).append('\n');
		cls.visitFieldsAndMethods(new ISeqConsumer<IFieldData>() {
			public void init(int count) { }
			public void accept(IFieldData f) {
				dumpField(sb, f);
			}
		}, new ISeqConsumer<IMethodData>() {
			public void init(int count) { }
			public void accept(IMethodData m) {
				dumpMethod(sb, m);
			}
		});
	}

	private static String innerInfo(InnerClsInfo i) {
		return i.getInnerCls() + "|" + i.getOuterCls() + "|" + i.getName() + "|0x" + Integer.toHexString(i.getAccessFlags());
	}

	private static void dumpField(StringBuilder sb, IFieldData f) {
		sb.append("F ").append(f.getName()).append('\t').append(f.getType()).append("\t0x")
				.append(Integer.toHexString(f.getAccessFlags())).append('\n');
		String sig = null;
		String cst = "none";
		List<IAnnotation> anns = new ArrayList<>();
		for (IJadxAttribute attr : f.getAttributes()) {
			if (attr instanceof SignatureAttr) {
				sig = ((SignatureAttr) attr).getSignature();
			} else if (attr instanceof EncodedValue) {
				cst = val((EncodedValue) attr);
			} else if (attr instanceof AnnotationsAttr) {
				anns.addAll(((AnnotationsAttr) attr).getAll());
			}
		}
		sb.append("FS sig=").append(sig).append(" const=").append(cst).append('\n');
		annotations(sb, "FA", anns);
		sb.append("FR ").append(refs(anns)).append('\n');
	}

	private static void dumpMethod(StringBuilder sb, IMethodData m) {
		JavaMethodRef ref = (JavaMethodRef) m.getMethodRef();
		ref.load();
		ICodeReader code = m.getCodeReader();
		sb.append("M ").append(ref.getName()).append(ref.getDescriptor()).append("\t0x")
				.append(Integer.toHexString(m.getAccessFlags())).append("\tcode=")
				.append(code == null ? "none" : String.valueOf(code.getCodeOffset())).append('\n');
		String sig = null;
		String throwsList = "none";
		String params = "none";
		String dflt = "none";
		List<IAnnotation> anns = new ArrayList<>();
		List<AnnotationsAttr> paramAnns = null;
		for (IJadxAttribute attr : m.getAttributes()) {
			if (attr instanceof SignatureAttr) {
				sig = ((SignatureAttr) attr).getSignature();
			} else if (attr instanceof ExceptionsAttr) {
				throwsList = "[" + String.join(",", ((ExceptionsAttr) attr).getList()) + "]";
			} else if (attr instanceof MethodParametersAttr) {
				params = "[" + ((MethodParametersAttr) attr).getList().stream()
						.map(p -> p.getName() + ":0x" + Integer.toHexString(p.getAccFlags()))
						.collect(Collectors.joining(",")) + "]";
			} else if (attr instanceof AnnotationDefaultAttr) {
				dflt = val(((AnnotationDefaultAttr) attr).getValue());
			} else if (attr instanceof AnnotationMethodParamsAttr) {
				paramAnns = ((AnnotationMethodParamsAttr) attr).getParamList();
			} else if (attr instanceof AnnotationsAttr) {
				anns.addAll(((AnnotationsAttr) attr).getAll());
			}
		}
		sb.append("MS sig=").append(sig).append(" throws=").append(throwsList)
				.append(" params=").append(params).append(" default=").append(dflt).append('\n');
		annotations(sb, "MA", anns);
		List<IAnnotation> allRefs = new ArrayList<>(anns);
		if (paramAnns != null) {
			for (int i = 0; i < paramAnns.size(); i++) {
				AnnotationsAttr p = paramAnns.get(i);
				List<IAnnotation> list = p == null ? List.of() : new ArrayList<>(p.getAll());
				annotations(sb, "MP" + i, list);
				allRefs.addAll(list);
			}
		}
		sb.append("MR ").append(refs(allRefs)).append('\n');
	}

	private static void annotations(StringBuilder sb, String prefix, Collection<IAnnotation> anns) {
		List<IAnnotation> sorted = new ArrayList<>(anns);
		sorted.sort(Comparator.comparing(IAnnotation::getAnnotationClass));
		for (IAnnotation a : sorted) {
			sb.append(prefix).append(' ').append(ann(a)).append('\n');
		}
	}

	private static String refs(Collection<IAnnotation> anns) {
		List<String> out = new ArrayList<>();
		for (IAnnotation a : anns) {
			annRefs(a, out);
		}
		out.sort(Comparator.naturalOrder());
		return "[" + String.join(",", out) + "]";
	}

	private static void annRefs(IAnnotation a, List<String> out) {
		out.add("Class:" + a.getAnnotationClass());
		for (EncodedValue v : a.getValues().values()) {
			valueRefs(v, out);
		}
	}

	@SuppressWarnings("unchecked")
	private static void valueRefs(EncodedValue v, List<String> out) {
		switch (v.getType()) {
			case ENCODED_TYPE:
				out.add("Class:" + v.getValue());
				break;
			case ENCODED_ENUM:
			case ENCODED_FIELD:
				IFieldRef f = (IFieldRef) v.getValue();
				out.add("Field:" + f.getParentClassType() + "->" + f.getName() + ":" + f.getType());
				break;
			case ENCODED_ARRAY:
				for (EncodedValue i : (List<EncodedValue>) v.getValue()) {
					valueRefs(i, out);
				}
				break;
			case ENCODED_ANNOTATION:
				annRefs((IAnnotation) v.getValue(), out);
				break;
			default:
				break;
		}
	}

	private static String ann(IAnnotation a) {
		StringBuilder sb = new StringBuilder();
		sb.append('@').append(a.getAnnotationClass()).append('[').append(a.getVisibility()).append("]{");
		boolean first = true;
		for (Map.Entry<String, EncodedValue> e : a.getValues().entrySet()) {
			if (!first) {
				sb.append(',');
			}
			first = false;
			sb.append(e.getKey()).append('=').append(val(e.getValue()));
		}
		return sb.append('}').toString();
	}

	@SuppressWarnings("unchecked")
	private static String val(EncodedValue v) {
		Object o = v.getValue();
		switch (v.getType()) {
			case ENCODED_NULL:
				return "null";
			case ENCODED_BOOLEAN:
				return "BOOL:" + o;
			case ENCODED_BYTE:
				return "BYTE:" + (int) (Byte) o;
			case ENCODED_SHORT:
				return "SHORT:" + (int) (Short) o;
			case ENCODED_CHAR:
				return "CHAR:" + (int) (Character) o;
			case ENCODED_INT:
				return "INT:" + o;
			case ENCODED_LONG:
				return "LONG:" + o;
			case ENCODED_FLOAT:
				return "FLOAT:0x" + Integer.toHexString(Float.floatToIntBits((Float) o));
			case ENCODED_DOUBLE:
				return "DOUBLE:0x" + Long.toHexString(Double.doubleToLongBits((Double) o));
			case ENCODED_STRING:
				return "STR:" + escape((String) o);
			case ENCODED_TYPE:
				return "TYPE:" + o;
			case ENCODED_ENUM:
			case ENCODED_FIELD:
				return "ENUM:" + fieldRef((IFieldRef) o);
			case ENCODED_METHOD_TYPE:
				return "MTYPE:" + proto((IMethodProto) o);
			case ENCODED_METHOD_HANDLE: {
				IMethodHandle h = (IMethodHandle) o;
				if (h.getType().isField()) {
					return "MH:" + h.getType() + ":" + fieldRef(h.getFieldRef());
				}
				IMethodRef m = h.getMethodRef();
				m.load();
				return "MH:" + h.getType() + ":" + methodRef(m);
			}
			case ENCODED_ARRAY:
				return "[" + ((List<EncodedValue>) o).stream().map(JavaInputDump::val).collect(Collectors.joining(",")) + "]";
			case ENCODED_ANNOTATION:
				return ann((IAnnotation) o);
			default:
				return v.getType() + ":" + o;
		}
	}

	private static String fieldRef(IFieldRef f) {
		return f.getParentClassType() + "->" + f.getName() + ":" + f.getType();
	}

	private static String proto(IMethodProto p) {
		return "(" + String.join("", p.getArgTypes()) + ")" + p.getReturnType();
	}

	private static String methodRef(IMethodRef m) {
		return m.getParentClassType() + "->" + m.getName() + proto(m);
	}

	private static String escape(String s) {
		if (s == null) {
			return "null";
		}
		StringBuilder sb = new StringBuilder("\"");
		for (int i = 0; i < s.length(); i++) {
			char c = s.charAt(i);
			if (c >= 0x20 && c <= 0x7e && c != '\\' && c != '"') {
				sb.append(c);
			} else {
				sb.append(String.format("\\u%04x", (int) c));
			}
		}
		return sb.append('"').toString();
	}

	// ---- instructions ----

	private static void dumpInsns(StringBuilder out, IClassData cls) {
		// like the decompiler core, load the class attributes before the methods (this fills the
		// attribute name cache that the call site resolution relies on)
		cls.getAttributes();
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
				JavaMethodRef ref = (JavaMethodRef) m.getMethodRef();
				ref.load();
				StringBuilder sb = new StringBuilder();
				sb.append("M ").append(ref.getParentClassType()).append("->").append(ref.getName()).append('(')
						.append(String.join("", ref.getArgTypes())).append(')').append(ref.getReturnType())
						.append(" regs=").append(code.getRegistersCount())
						.append(" units=").append(code.getUnitsCount())
						.append(" args=").append(code.getArgsStartReg()).append('\n');
				try {
					code.visitInstructions(insn -> dump(sb, insn));
					for (ITry t : code.getTries()) {
						sb.append("T ").append(t.getStartOffset()).append(' ').append(t.getEndOffset()).append(" [");
						var c = t.getCatch();
						String[] types = c.getTypes();
						int[] handlers = c.getHandlers();
						for (int i = 0; i < types.length; i++) {
							sb.append(i == 0 ? "" : ",").append(types[i]).append("->").append(handlers[i]);
						}
						sb.append("] all=").append(c.getCatchAllHandler()).append('\n');
					}
					IDebugInfo di = code.getDebugInfo();
					if (di == null) {
						sb.append("D none\n");
					} else {
						sb.append("D lines=");
						new TreeMap<>(di.getSourceLineMapping()).forEach((k, v) -> sb.append(k).append('=').append(v).append(','));
						sb.append('\n');
						for (ILocalVar v : di.getLocalVars()) {
							sb.append("V ").append(v.getRegNum()).append(' ').append(v.getName()).append(' ')
									.append(v.getType()).append(' ').append(v.getSignature()).append(' ')
									.append(v.getStartOffset()).append(' ').append(v.getEndOffset()).append('\n');
						}
					}
				} catch (Exception e) {
					sb.setLength(sb.indexOf("\n") + 1);
					sb.append("E LOAD_ERROR\n");
				}
				out.append(sb);
			}
		});
	}

	private static void dump(StringBuilder sb, InsnData insn) {
		JavaInsnData d = (JavaInsnData) insn;
		Arrays.fill(d.getRegsArray(), SENT_REG);
		d.setLiteral(SENT_LIT);
		d.setTarget(SENT_INT);
		d.setIndex(SENT_INT);
		insn.decode();
		Opcode op = insn.getOpcode();
		sb.append("I ").append(Integer.toHexString(insn.getOffset())).append(' ').append(op);
		int[] arr = d.getRegsArray();
		int n = Math.max(0, Math.min(insn.getRegsCount(), arr.length));
		int last = -1;
		for (int i = 0; i < n; i++) {
			if (arr[i] != SENT_REG) {
				last = i;
			}
		}
		sb.append(" r=[");
		for (int i = 0; i <= last; i++) {
			sb.append(i == 0 ? "" : ",").append(arr[i] == SENT_REG ? "?" : String.valueOf(arr[i]));
		}
		sb.append(']');
		long lit = insn.getLiteral();
		int target = insn.getTarget();
		int index = insn.getIndex();
		sb.append(" l=").append(lit == SENT_LIT ? 0 : lit);
		sb.append(" t=").append(target == SENT_INT ? 0 : target);
		sb.append(" i=").append(index == SENT_INT ? 0 : index).append(" it=").append(insn.getIndexType());
		String name = op.name();
		if (name.startsWith("INVOKE_")) {
			sb.append(" res=").append(insn.getResultReg());
		}
		Object p = insn.getPayload();
		if (p instanceof ISwitchPayload) {
			ISwitchPayload sp = (ISwitchPayload) p;
			sb.append(" switch keys=").append(Arrays.toString(sp.getKeys()))
					.append(" targets=").append(Arrays.toString(sp.getTargets()));
		} else if (p instanceof ICallSite) {
			ICallSite cs = (ICallSite) p;
			cs.load();
			sb.append(" cs=[").append(cs.getValues().stream().map(JavaInputDump::val).collect(Collectors.joining(","))).append(']');
		}
		sb.append('\n');
	}
}
