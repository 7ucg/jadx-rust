package jadx.core.codegen;

import java.io.File;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import jadx.api.ICodeWriter;
import jadx.api.JadxArgs;
import jadx.api.JadxDecompiler;
import jadx.api.impl.SimpleCodeWriter;
import jadx.core.dex.attributes.AFlag;
import jadx.core.dex.instructions.InsnType;
import jadx.core.dex.instructions.args.CodeVar;
import jadx.core.dex.instructions.args.InsnArg;
import jadx.core.dex.instructions.args.InsnWrapArg;
import jadx.core.dex.instructions.args.RegisterArg;
import jadx.core.dex.instructions.args.SSAVar;
import jadx.core.dex.instructions.mods.ConstructorInsn;
import jadx.core.dex.nodes.BlockNode;
import jadx.core.dex.nodes.ClassNode;
import jadx.core.dex.nodes.InsnNode;
import jadx.core.dex.nodes.MethodNode;
import jadx.core.dex.nodes.RootNode;
import jadx.core.dex.visitors.ApplyVariableNames;
import jadx.core.dex.visitors.ConstInlineVisitor;
import jadx.core.dex.visitors.ConstructorVisitor;
import jadx.core.dex.visitors.InitCodeVariables;
import jadx.core.dex.visitors.ModVisitor;
import jadx.core.dex.visitors.MoveInlineVisitor;
import jadx.core.dex.visitors.AttachTryCatchVisitor;
import jadx.core.dex.visitors.ProcessInstructionsVisitor;
import jadx.core.dex.visitors.blocks.BlockFinisher;
import jadx.core.dex.visitors.blocks.BlockProcessor;
import jadx.core.dex.visitors.blocks.BlockSplitter;
import jadx.core.dex.visitors.shrink.CodeShrinkVisitor;
import jadx.core.dex.visitors.ssa.SSATransform;
import jadx.core.dex.visitors.typeinference.FinishTypeInference;
import jadx.core.dex.visitors.typeinference.FixTypesVisitor;
import jadx.core.dex.visitors.typeinference.TypeInferenceVisitor;

/**
 * Oracle helper: runs the reference pipeline up to CodeShrinkVisitor (the stages of BlockDump with
 * STAGE=11) and prints, for every block instruction and every instruction wrapped into one, the
 * code string the reference InsnGen produces.
 *
 * The class lives in the codegen package to reach the package private generator entry points.
 *
 * Env: NAMES=1 marks the first assignment of every variable as its declaration (as the fallback
 * method generator does) and applies ApplyVariableNames beforehand.
 */
public class InsnCodeDump {

	public static void main(String[] args) throws Exception {
		JadxArgs jargs = new JadxArgs();
		jargs.setInputFile(new File(args[0]));
		jargs.setSkipResources(true);
		boolean extractFinally = "1".equals(System.getenv("JADX_RS_FINALLY"));
		if (!extractFinally) {
			jargs.setExtractFinally(false);
		}
		boolean names = "1".equals(System.getenv("NAMES"));
		StringBuilder sb = new StringBuilder();
		Map<ClassNode, ClassGen> topGens = new HashMap<>();
		try (JadxDecompiler jadx = new JadxDecompiler(jargs)) {
			jadx.load();
			RootNode root = jadx.getRoot();
			for (ClassNode cls : root.getClasses()) {
				ClassNode top = cls.getTopParentClass();
				ClassGen topGen = topGens.computeIfAbsent(top, c -> {
					ClassGen g = new ClassGen(c, jargs);
					g.setBodyGenStarted(true);
					return g;
				});
				ClassGen clsGen = cls == top ? topGen : new ClassGen(cls, topGen);
				clsGen.setBodyGenStarted(true);
				for (MethodNode mth : cls.getMethods()) {
					if (mth.isNoCode()) {
						continue;
					}
					try {
						mth.load();
						new AttachTryCatchVisitor().visit(mth);
						new ProcessInstructionsVisitor().visit(mth);
						new BlockSplitter().visit(mth);
						new BlockProcessor().visit(mth);
						new BlockFinisher().visit(mth);
						new SSATransform().visit(mth);
						new MoveInlineVisitor().visit(mth);
						new ConstructorVisitor().visit(mth);
						new InitCodeVariables().visit(mth);
						if (extractFinally) {
							new jadx.core.dex.visitors.finaly.MarkFinallyVisitor().visit(mth);
						}
						new ConstInlineVisitor().visit(mth);
						TypeInferenceVisitor ti = new TypeInferenceVisitor();
						ti.init(root);
						ti.visit(mth);
						FixTypesVisitor fix = new FixTypesVisitor();
						fix.init(root);
						fix.visit(mth);
						new FinishTypeInference().visit(mth);
						new ModVisitor().visit(mth);
						new CodeShrinkVisitor().visit(mth);
						if (names) {
							prepareNames(root, mth);
						}
					} catch (Throwable e) {
						sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append(" LOAD_ERROR\n");
						System.out.print(sb);
						sb.setLength(0);
						continue;
					}
					dump(sb, jargs, clsGen, mth, names);
					System.out.print(sb);
					sb.setLength(0);
				}
			}
		}
	}

	/** Declare variables at their first assignment (in block order) and guess variable names. */
	private static void prepareNames(RootNode root, MethodNode mth) throws Exception {
		for (SSAVar v : mth.getSVars()) {
			v.getCodeVar().setDeclared(true);
		}
		ApplyVariableNames apply = new ApplyVariableNames();
		apply.init(root);
		apply.visit(mth);
		for (SSAVar v : mth.getSVars()) {
			v.getCodeVar().setDeclared(false);
		}
		RegisterArg thisArg = mth.getThisArg();
		if (thisArg != null && thisArg.getSVar() != null) {
			thisArg.getSVar().getCodeVar().setDeclared(true);
		}
		for (RegisterArg arg : mth.getArgRegs()) {
			if (arg.getSVar() != null) {
				arg.getSVar().getCodeVar().setDeclared(true);
			}
		}
		for (BlockNode block : mth.getBasicBlocks()) {
			for (InsnNode insn : block.getInstructions()) {
				if (insn.contains(AFlag.DONT_GENERATE)) {
					continue;
				}
				if (insn.getResult() != null) {
					CodeVar codeVar = insn.getResult().getSVar().getCodeVar();
					if (!codeVar.isDeclared()) {
						insn.add(AFlag.DECLARE_VAR);
						codeVar.setDeclared(true);
					}
				}
			}
		}
	}

	private static void dump(StringBuilder sb, JadxArgs jargs, ClassGen clsGen, MethodNode mth, boolean names) {
		sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append('\n');
		MethodGen mgen = new MethodGen(clsGen, mth);
		InsnGen gen = new InsnGen(mgen, false);
		if (names) {
			// method definition assigns the argument names
			for (RegisterArg arg : mth.getArgRegs()) {
				if (arg.getSVar() != null) {
					mgen.getNameGen().assignArg(arg.getSVar().getCodeVar());
				}
			}
		}
		for (BlockNode block : mth.getBasicBlocks()) {
			int i = 0;
			for (InsnNode insn : block.getInstructions()) {
				String loc = "b" + block.getCId() + "i" + i++;
				statement(sb, jargs, gen, loc, insn);
				wrapped(sb, jargs, gen, loc, insn);
			}
		}
	}

	private static boolean hasAnonymousCtor(InsnGen gen, InsnNode insn) {
		if (insn.getType() == InsnType.CONSTRUCTOR) {
			ClassNode cls = gen.root.resolveClass(((ConstructorInsn) insn).getClassType());
			if (cls != null && cls.isAnonymous()) {
				return true;
			}
		}
		for (InsnArg arg : insn.getArguments()) {
			if (arg.isInsnWrap() && hasAnonymousCtor(gen, ((InsnWrapArg) arg).getWrapInsn())) {
				return true;
			}
		}
		return false;
	}

	private static void statement(StringBuilder sb, JadxArgs jargs, InsnGen gen, String loc, InsnNode insn) {
		sb.append("S ").append(loc).append(' ').append(insn.getType()).append(' ');
		if (hasAnonymousCtor(gen, insn)) {
			sb.append("<ANON>\n");
			return;
		}
		ICodeWriter code = new SimpleCodeWriter(jargs);
		try {
			gen.makeInsn(insn, code);
			sb.append(clean(code.getCodeStr())).append('\n');
		} catch (Throwable e) {
			sb.append("<ERR>\n");
		}
	}

	private static void wrapped(StringBuilder sb, JadxArgs jargs, InsnGen gen, String loc, InsnNode insn) {
		List<InsnArg> args = (List<InsnArg>) insn.getArguments();
		for (int k = 0; k < args.size(); k++) {
			InsnArg arg = args.get(k);
			if (!arg.isInsnWrap()) {
				continue;
			}
			InsnNode wrap = ((InsnWrapArg) arg).getWrapInsn();
			String wloc = loc + "." + k;
			sb.append("W ").append(wloc).append(' ').append(wrap.getType()).append(' ');
			if (hasAnonymousCtor(gen, wrap)) {
				sb.append("<ANON>\n");
			} else {
				ICodeWriter code = new SimpleCodeWriter(jargs);
				try {
					gen.makeInsn(wrap, code, InsnGen.Flags.BODY_ONLY);
					sb.append(clean(code.getCodeStr())).append('\n');
				} catch (Throwable e) {
					sb.append("<ERR>\n");
				}
			}
			wrapped(sb, jargs, gen, wloc, wrap);
		}
	}

	private static String clean(String str) {
		String s = str.startsWith("\n") ? str.substring(1) : str;
		return s.replace("\r", "").replace("\n", "<NL>");
	}
}
