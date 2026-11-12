package corpus;

public class Arrays {
    static int[][] matrix(int n) {
        int[][] m = new int[n][n];
        for (int i = 0; i < n; i++) m[i][i] = 1;
        return m;
    }

    static String[][][] cube() { return new String[2][3][4]; }
    static int[][] partial() { return new int[3][]; }
    static int[] init() { return new int[] {1, 2, 3, 40000, -5}; }
    static String[] strs() { return new String[] {"a", "b", null}; }
    static long[] longs() { return new long[] {1L, 2L, 1L << 40}; }
    static boolean[] bools() { return new boolean[4]; }
    static byte[] bytes() { return new byte[] {1, -2, 3}; }
    static char[] chars() { return new char[] {'a', 'b'}; }
    static short[] shorts() { return new short[2]; }
    static float[] floats() { return new float[] {1f, 2.5f}; }
    static double[] doubles() { return new double[] {1.0, 2.5}; }
    static Object[] objs(Object o) { return new Object[] {o, 1, "s", 2L}; }
    static int sum(int... xs) { int s = 0; for (int x : xs) s += x; return s; }
    static int len(Object[] a) { return a.length; }
    static boolean isArr(Object o) { return o instanceof int[]; }
    static Object cast(Object o) { return (String[]) o; }
    static void store(Object[] a, Object o, int i) { a[i] = o; }
    static byte bload(byte[] b, int i) { return b[i]; }
    static void bstore(boolean[] b, byte[] c, int i) { b[i] = true; c[i] = 3; }
    static char cload(char[] c) { return c[0]; }
    static short sload(short[] s) { return s[1]; }
}
