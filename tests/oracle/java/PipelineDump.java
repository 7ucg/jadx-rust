import java.io.File;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import jadx.api.JadxArgs;
import jadx.api.JadxDecompiler;
import jadx.core.dex.attributes.AFlag;
import jadx.core.dex.attributes.AType;
import jadx.core.dex.nodes.ClassNode;
import jadx.core.dex.nodes.MethodNode;
import jadx.core.dex.nodes.RootNode;
import jadx.core.dex.visitors.IDexTreeVisitor;

/**
 * Oracle helper: runs the real pass list of the reference class by class (dependencies first, like
 * the code generation entry point does, but without generating code) and dumps the region tree of
 * every method in the format of `jadx-rs insn-regions`.
 *
 * The stage selects which passes run: every pass listed in {@link #STAGES} runs only when its stage
 * is lower or equal to the requested one, all other passes always run. Passes keep their order of
 * the reference pass list. Usage: PipelineDump file stage [--list]
 */
public class PipelineDump {
	/** Stage of the passes ported so far, key is the simple class name (`#2` for a second occurrence). */
	static final Map<String, Integer> STAGES = new HashMap<>();

	static {
		String[][] table = {
				{ "ConstInlineVisitor", "6" }, { "TypeInferenceVisitor", "7" }, { "FixTypesVisitor", "8" },
				{ "FinishTypeInference", "9" }, { "ModVisitor", "10" }, { "CodeShrinkVisitor", "11" },
				{ "RegionMakerVisitor", "12" }, { "IfRegionVisitor", "13" }, { "ReturnVisitor", "14" },
				{ "CleanRegions", "15" }, { "CodeShrinkVisitor#2", "16" }, { "CheckRegions", "17" },
				{ "LoopRegionVisitor", "18" }, { "SwitchBreakVisitor", "19" },
				// passes after stage 19 (stage numbers in the order they were ported)
				{ "ReplaceNewArray", "20" }, { "DeboxingVisitor", "21" }, { "ShadowFieldVisitor", "22" },
				{ "GenericTypesVisitor", "23" }, { "ProcessKotlinInternals", "24" }, { "SwitchOverStringVisitor", "25" },
				{ "MethodThrowsVisitor", "26" }, { "MethodInvokeVisitor", "27" }, { "SimplifyVisitor", "28" },
				{ "FixInstructionsVisitor", "29" }, { "ProcessVariables", "30" }, { "ApplyVariableNames", "31" },
				{ "PrepareForCodeGen", "32" }, { "AnonymousClassVisitor", "33" }, { "ExtractFieldInit", "34" },
				{ "ClassModifier", "35" }, { "EnumVisitor", "36" }, { "InlineMethods", "37" }, { "FixSwitchOverEnum", "38" },
				{ "NonFinalResIdsVisitor", "39" }, { "FixAccessModifiers", "40" },
				{ "MarkMethodsForInline", "41" }
		};
		for (String[] e : table) {
			STAGES.put(e[0], Integer.parseInt(e[1]));
		}
	}

	public static void main(String[] args) throws Exception {
		JadxArgs jargs = new JadxArgs();
		jargs.setInputFile(new File(args[0]));
		jargs.setSkipResources(true);
		jargs.setDebugInfo(false);
		// the Kotlin metadata plugin renames variables, it is not part of the core
		jargs.getDisabledPasses().add("KotlinMetadataDecompile");
		boolean extractFinally = "1".equals(System.getenv("JADX_RS_FINALLY"));
		jargs.setExtractFinally(extractFinally);
		int stage = Integer.parseInt(args[1]);
		InsnNodeDump.SSA = true;
		RegionDump.WITH_VARS = Integer.parseInt(args[1]) >= 30;
		try (JadxDecompiler jadx = new JadxDecompiler(jargs)) {
			jadx.load();
			RootNode root = jadx.getRoot();
			List<IDexTreeVisitor> passes = root.getPasses();
			if (args.length > 2 && args[2].equals("--list")) {
				for (IDexTreeVisitor p : passes) {
					System.out.println(p.getClass().getSimpleName() + " " + p.getName());
				}
				return;
			}
			Map<String, Integer> seen = new HashMap<>();
			List<IDexTreeVisitor> keep = new ArrayList<>();
			for (IDexTreeVisitor p : passes) {
				String name = p.getClass().getSimpleName();
				int n = seen.merge(name, 1, Integer::sum);
				String key = n == 1 ? name : name + "#" + n;
				Integer st = STAGES.get(key);
				if (st == null || st <= stage) {
					keep.add(p);
				}
			}
			passes.clear();
			passes.addAll(keep);
			if (System.getenv("LOG_ORDER") != null) {
				passes.add(0, new jadx.core.dex.visitors.AbstractVisitor() {
					@Override
					public boolean visit(ClassNode cls) {
						if (!cls.isInner()) {
							System.err.println("PROC " + cls.getClassInfo().getRawName());
						}
						return false;
					}
				});
			}
			StringBuilder sb = new StringBuilder();
			for (ClassNode cls : new ArrayList<>(root.getClasses())) {
				if (cls.isInner()) {
					continue;
				}
				try {
					if (cls.contains(AFlag.DONT_GENERATE)) {
						root.getProcessClasses().forceProcess(cls);
						continue;
					}
					for (ClassNode dep : cls.getDependencies()) {
						root.getProcessClasses().forceProcess(dep);
					}
					if (!cls.getCodegenDeps().isEmpty()) {
						// like ProcessClass.generateCode: the class first, then the classes needed for its code
						root.getProcessClasses().forceProcess(cls);
						for (ClassNode dep : cls.getCodegenDeps()) {
							root.getProcessClasses().forceProcess(dep);
						}
					}
					root.getProcessClasses().forceProcess(cls);
				} catch (Throwable e) {
					// reported through the method errors
				}
			}
			for (ClassNode cls : root.getClasses()) {
				if (stage >= 33) {
					ClassStateDump.dump(sb, cls);
				}
				for (MethodNode mth : cls.getMethods()) {
					if (mth.isNoCode()) {
						continue;
					}
					if (mth.contains(AType.JADX_ERROR) || mth.getBasicBlocks() == null && mth.getRegion() == null) {
						sb.append("M ").append(mth.getMethodInfo().getRawFullId()).append(" LOAD_ERROR\n");
					} else {
						RegionDump.dump(sb, mth);
					}
					System.out.print(sb);
					sb.setLength(0);
				}
				System.out.print(sb);
				sb.setLength(0);
			}
		}
	}
}
