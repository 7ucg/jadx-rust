import java.nio.file.Path;
import java.util.List;

import jadx.api.plugins.input.data.IClassData;
import jadx.plugins.input.dex.DexFileLoader;
import jadx.plugins.input.dex.DexInputOptions;
import jadx.plugins.input.dex.DexReader;

/**
 * Oracle helper: dumps one line per class using the reference dex reader, in the same format as
 * `jadx-rs dex-classes`: type, access flags, super type, interfaces.
 * Run with the jadx distribution jars on the classpath.
 */
public class DexClassDump {
	public static void main(String[] args) throws Exception {
		DexInputOptions opts = new DexInputOptions();
		opts.registerOptions();
		DexFileLoader loader = new DexFileLoader(opts);
		List<DexReader> readers = loader.collectDexFiles(List.of(Path.of(args[0])));
		boolean counts = args.length > 1 && args[1].equals("--counts");
		for (DexReader reader : readers) {
			if (counts) {
				var h = reader.getHeader();
				System.out.println("COUNTS\ttypes=" + h.getTypeIdsSize() + "\tprotos=" + h.getProtoIdsSize()
						+ "\tfields=" + h.getFieldIdsSize() + "\tmethods=" + h.getMethodIdsSize()
						+ "\tclasses=" + h.getClassDefsSize());
				continue;
			}
			reader.visitClasses(cls -> {
				String sup = cls.getSuperType();
				System.out.println(cls.getType() + "\t0x" + Integer.toHexString(cls.getAccessFlags())
						+ "\t" + (sup == null ? "null" : sup)
						+ "\t" + String.join(",", cls.getInterfacesTypes()));
			});
		}
	}
}
