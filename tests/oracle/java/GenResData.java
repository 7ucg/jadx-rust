import java.lang.reflect.Field;
import java.util.Map;
import java.util.TreeMap;

import jadx.api.JadxArgs;
import jadx.core.xmlgen.ManifestAttributes;

/**
 * Dev tool: prints the android attribute enum/flag table of the reference (parsed from its bundled
 * attrs.xml and attrs_manifest.xml) as one line per attribute:
 * <pre>
 * android:name TAB E|F TAB key=value SPACE key=value ...
 * </pre>
 * The Rust crate embeds this text, so the table cannot drift from the reference.
 */
public class GenResData {
	public static void main(String[] args) throws Exception {
		ManifestAttributes attrs = new ManifestAttributes(new JadxArgs().getSecurity());
		Field f = ManifestAttributes.class.getDeclaredField("attrMap");
		f.setAccessible(true);
		@SuppressWarnings("unchecked")
		Map<String, Object> map = new TreeMap<>((Map<String, Object>) f.get(attrs));
		for (Map.Entry<String, Object> e : map.entrySet()) {
			Object attr = e.getValue();
			Field typeField = attr.getClass().getDeclaredField("type");
			typeField.setAccessible(true);
			Field valuesField = attr.getClass().getDeclaredField("values");
			valuesField.setAccessible(true);
			String type = String.valueOf(typeField.get(attr)).equals("ENUM") ? "E" : "F";
			@SuppressWarnings("unchecked")
			Map<Long, String> values = (Map<Long, String>) valuesField.get(attr);
			StringBuilder sb = new StringBuilder();
			for (Map.Entry<Long, String> v : values.entrySet()) {
				if (sb.length() != 0) {
					sb.append(' ');
				}
				sb.append(v.getKey()).append('=').append(v.getValue());
			}
			System.out.println(e.getKey() + "\t" + type + "\t" + sb);
		}
	}
}
