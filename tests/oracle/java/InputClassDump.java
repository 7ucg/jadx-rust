import java.io.File;
import java.util.ArrayList;
import java.util.List;

import jadx.api.JadxArgs;
import jadx.api.JadxDecompiler;
import jadx.core.dex.nodes.ClassNode;

/**
 * Oracle helper: loads the inputs with all reference input plugins (dex input, java input,
 * xapk/apks/apkm inputs) and prints one `CLS<TAB>name<TAB>source` line per class of the root,
 * in the format of `jadx-rs input-classes` (plus the `CLS` marker that separates the lines from
 * log output). Classes generated from resources have no input file and are skipped.
 */
public class InputClassDump {
	public static void main(String[] args) throws Exception {
		JadxArgs jargs = new JadxArgs();
		List<File> files = new ArrayList<>();
		for (String a : args) {
			files.add(new File(a));
		}
		jargs.setInputFiles(files);
		jargs.setSkipResources(true);
		try (JadxDecompiler jadx = new JadxDecompiler(jargs)) {
			jadx.load();
			for (ClassNode cls : jadx.getRoot().getClasses()) {
				String src = cls.getInputFileName();
				if (src == null || src.isEmpty() || src.equals("synthetic")) {
					continue;
				}
				System.out.println("CLS\t" + cls.getRawName() + "\t" + src);
			}
		}
	}
}
