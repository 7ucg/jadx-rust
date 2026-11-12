import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.Collectors;
import java.util.stream.Stream;

import com.android.tools.smali.smali.Smali;
import com.android.tools.smali.smali.SmaliOptions;

/**
 * Oracle helper: assembles every .smali file under a directory into one dex file using the
 * reference assembler. Usage: SmaliAssemble <dir> <out.dex>
 */
public class SmaliAssemble {
	public static void main(String[] args) throws Exception {
		List<String> files;
		try (Stream<Path> s = Files.walk(Path.of(args[0]))) {
			files = s.filter(p -> p.toString().endsWith(".smali")).sorted().map(Path::toString).collect(Collectors.toList());
		}
		SmaliOptions opts = new SmaliOptions();
		opts.apiLevel = 34;
		opts.outputDexFile = args[1];
		if (!Smali.assemble(opts, files)) {
			System.err.println("assemble failed");
			System.exit(1);
		}
	}
}
