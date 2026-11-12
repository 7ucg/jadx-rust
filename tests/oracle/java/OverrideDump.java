import java.io.File;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.stream.Collectors;

import jadx.api.JadxArgs;
import jadx.api.JadxDecompiler;
import jadx.core.dex.attributes.AType;
import jadx.core.dex.attributes.nodes.MethodOverrideAttr;
import jadx.core.dex.nodes.ClassNode;
import jadx.core.dex.nodes.MethodNode;
import jadx.core.dex.nodes.RootNode;

/** Oracle helper: prints the base methods of the method override attribute of every method. */
public class OverrideDump {
	public static void main(String[] args) throws Exception {
		JadxArgs jargs = new JadxArgs();
		jargs.setInputFile(new File(args[0]));
		jargs.setSkipResources(true);
		List<String> out = new ArrayList<>();
		try (JadxDecompiler jadx = new JadxDecompiler(jargs)) {
			jadx.load();
			RootNode root = jadx.getRoot();
			for (ClassNode cls : root.getClasses()) {
				for (MethodNode mth : cls.getMethods()) {
					MethodOverrideAttr a = mth.get(AType.METHOD_OVERRIDE);
					if (a == null) {
						continue;
					}
					List<String> bases = a.getBaseMethods().stream()
							.map(m -> m.getMethodInfo().getDeclClass().getRawName() + "." + m.getMethodInfo().getShortId())
							.sorted().collect(Collectors.toList());
					out.add("O " + mth.getMethodInfo().getRawFullId() + " base=" + bases);
				}
			}
		}
		Collections.sort(out);
		out.forEach(System.out::println);
	}
}
