package corpus;

public class Consts {
    public static final int I = 42;
    public static final int NEG = -70000;
    public static final long L = 1L << 50;
    public static final float F = 3.14f;
    public static final double D = 2.718281828;
    public static final boolean Z = true;
    public static final byte B = -3;
    public static final short S = 300;
    public static final char C = 'q';
    public static final String STR = "hello";
    public static final String UNI = "café € \u0000 nul 😀 tab\t";
    public static final String EMPTY = "";
    public static final double NAN = Double.NaN;
    public static final float INF = Float.POSITIVE_INFINITY;
    public static final String LONG_STR = "0123456789".repeat(1) + "abcdefghij";
    static int notConst = Integer.parseInt("5");
    final int instConst = 9;
    static final Object OBJ = new Object();

    static String uni() { return "éè 😀 \u0000"; }
    static Class<?> cls() { return Consts.class; }
    static Class<?> prim() { return int.class; }
    static Class<?> arrCls() { return String[][].class; }
}
