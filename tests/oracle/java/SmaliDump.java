import java.io.OutputStreamWriter;
import java.io.PrintWriter;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipFile;

import com.android.tools.smali.baksmali.Adaptors.ClassDefinition;
import com.android.tools.smali.baksmali.BaksmaliOptions;
import com.android.tools.smali.baksmali.formatter.BaksmaliWriter;
import com.android.tools.smali.dexlib2.dexbacked.DexBackedClassDef;
import com.android.tools.smali.dexlib2.dexbacked.DexBackedDexFile;

/**
 * Oracle helper: dumps baksmali output for every class of every dex in the given file.
 * Usage: SmaliDump <input.dex|.apk> [--debug]   (debug info is off unless --debug is given)
 */
public class SmaliDump {
	public static void main(String[] args) throws Exception {
		boolean debug = args.length > 1 && args[1].equals("--debug");
		Writer out = new java.io.BufferedWriter(new OutputStreamWriter(System.out, StandardCharsets.UTF_8), 1 << 16);
		Path path = Path.of(args[0]);
		byte[] head = Files.readAllBytes(path);
		if (head.length > 4 && head[0] == 'd' && head[1] == 'e' && head[2] == 'x') {
			dump(head, debug, out);
		} else {
			try (ZipFile zip = new ZipFile(path.toFile())) {
				for (var en = zip.entries(); en.hasMoreElements();) {
					ZipEntry e = en.nextElement();
					if (e.isDirectory()) {
						continue;
					}
					byte[] data = zip.getInputStream(e).readAllBytes();
					if (data.length > 4 && data[0] == 'd' && data[1] == 'e' && data[2] == 'x' && data[3] == '\n') {
						dump(data, debug, out);
					}
				}
			}
		}
		out.flush();
	}

	private static void dump(byte[] buf, boolean debug, Writer out) throws Exception {
		BaksmaliOptions options = new BaksmaliOptions();
		options.debugInfo = debug;
		DexBackedDexFile dex = new DexBackedDexFile(null, buf);
		for (DexBackedClassDef cls : dex.getClasses()) {
			StringBuilder sb = new StringBuilder();
			java.io.StringWriter sw = new java.io.StringWriter();
			try (BaksmaliWriter w = new BaksmaliWriter(sw)) {
				new ClassDefinition(options, cls).writeTo(w);
			}
			out.write(sw.toString().replace("\r\n", "\n"));
			out.write("\n#---end-class---\n");
		}
	}
}
