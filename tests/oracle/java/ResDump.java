import java.io.File;
import java.lang.reflect.Field;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.TreeMap;

import jadx.api.JadxArgs;
import jadx.api.JadxDecompiler;
import jadx.api.ResourceFile;
import jadx.api.ResourceType;
import jadx.core.dex.info.ClassInfo;
import jadx.core.dex.info.InfoStorage;
import jadx.core.dex.instructions.args.ArgType;
import jadx.core.dex.nodes.FieldNode;
import jadx.core.dex.nodes.IFieldInfoRef;
import jadx.core.xmlgen.ResContainer;

/**
 * Oracle helper: decodes the text resources (AndroidManifest.xml, binary xml files and the
 * resources.arsc tables) of an APK with the reference decompiler and writes them below an output
 * directory, one file per resource, named like the reference resource saver names them. The
 * resource id to name table is written as "resnames.txt" (one "0x%08x type/name" line per id).
 *
 * When a third argument is given, the tables that come from the decompiled code are written to
 * that directory: the class alias table (renamed classes, inner class names) and the names of the
 * constant fields that carry resource ids. The Rust decoder reads them to resolve class names in
 * the XML and to name resources the same way.
 *
 * Usage: ResDump input.apk outDir [auxDir]
 */
public class ResDump {
	public static void main(String[] args) throws Exception {
		File out = new File(args[1]);
		JadxArgs jargs = new JadxArgs();
		jargs.setInputFile(new File(args[0]));
		jargs.setSkipSources(true);
		jargs.setSkipResources(false);
		try (JadxDecompiler jadx = new JadxDecompiler(jargs)) {
			jadx.load();
			List<ResourceFile> resources = new ArrayList<>(jadx.getResources());
			// the manifest first, like the reference saver
			for (ResourceFile rf : resources) {
				if (rf.getType() == ResourceType.MANIFEST) {
					save(rf.loadContent(), out);
					break;
				}
			}
			for (ResourceFile rf : resources) {
				ResourceType type = rf.getType();
				if (type == ResourceType.MANIFEST) {
					continue;
				}
				if (type == ResourceType.XML || type == ResourceType.ARSC) {
					save(rf.loadContent(), out);
				}
			}
			Map<Integer, String> names = new TreeMap<>(Integer::compareUnsigned);
			names.putAll(jadx.getRoot().getConstValues().getResourcesNames());
			StringBuilder sb = new StringBuilder();
			for (Map.Entry<Integer, String> e : names.entrySet()) {
				sb.append(String.format("0x%08x %s%n", e.getKey(), e.getValue()));
			}
			write(new File(out, "resnames.txt"), sb.toString());
			if (args.length > 2) {
				File aux = new File(args[2]);
				writeClassAliases(jadx, new File(aux, "class-aliases.txt"));
				writeCodeNames(jadx, new File(aux, "code-names.txt"));
			}
		}
	}

	/**
	 * Names of the constant fields of the app code by value (int constants that other code does
	 * not use any more, e.g. the fields of the app resource class), one "0x%08x TAB name" line
	 * each. Fields of generated classes are left out.
	 */
	private static void writeCodeNames(JadxDecompiler jadx, File file) throws Exception {
		List<String> lines = new ArrayList<>();
		for (Map.Entry<Object, IFieldInfoRef> e : jadx.getRoot().getConstValues().getGlobalConstFields().entrySet()) {
			if (e.getKey() instanceof Integer && e.getValue() instanceof FieldNode) {
				FieldNode fld = (FieldNode) e.getValue();
				if (!fld.getTopParentClass().isSynthetic()) {
					lines.add(String.format("0x%08x\t%s", (Integer) e.getKey(), fld.getName()));
				}
			}
		}
		Collections.sort(lines);
		write(file, String.join("\n", lines) + "\n");
	}

	/**
	 * Classes whose alias name (as used in decoded XML) differs from the raw name, one
	 * "raw TAB alias" line each. Inner classes use '.' in the alias, renamed classes their new name.
	 */
	private static void writeClassAliases(JadxDecompiler jadx, File file) throws Exception {
		Field f = InfoStorage.class.getDeclaredField("classes");
		f.setAccessible(true);
		@SuppressWarnings("unchecked")
		Map<ArgType, ClassInfo> classes = (Map<ArgType, ClassInfo>) f.get(jadx.getRoot().getInfoStorage());
		List<String> lines = new ArrayList<>();
		for (ClassInfo ci : classes.values()) {
			String raw = ci.getType().getObject();
			String alias = ci.getAliasFullName();
			if (!raw.equals(alias)) {
				lines.add(raw + "\t" + alias);
			}
		}
		Collections.sort(lines);
		write(file, String.join("\n", lines) + "\n");
	}

	private static void save(ResContainer rc, File out) throws Exception {
		switch (rc.getDataType()) {
			case TEXT:
				write(new File(out, rc.getFileName()), rc.getText().getCodeStr() + "\n");
				break;
			case RES_TABLE:
				write(new File(out, "res/values/public.xml"), rc.getText().getCodeStr() + "\n");
				for (ResContainer sub : rc.getSubFiles()) {
					save(sub, out);
				}
				break;
			default:
				// binary content is not part of the comparison
				break;
		}
	}

	private static void write(File file, String text) throws Exception {
		Path p = file.toPath();
		Files.createDirectories(p.getParent());
		Files.write(p, text.getBytes(StandardCharsets.UTF_8));
	}
}
