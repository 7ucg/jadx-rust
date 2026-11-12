package corpus;

public class Arith {
    long lf = 5L;
    double df = 1.5;
    static long sl;
    static double sd;
    int[] ia = new int[10];
    long[] la = new long[10];

    static long longMath(long a, long b) {
        long r = a * b + (a << 3) - (b >> 2) + (a >>> 5);
        r ^= a & b | ~b;
        r %= 7;
        return r / (b | 1);
    }

    static double doubleMath(double a, double b, float f) {
        double r = a * b - a / b + a % b + f;
        r = -r;
        return r + (float) r + (long) r + (int) r + (short) (int) r + (char) (int) r + (byte) (int) r;
    }

    static int cmp(long a, long b, double c, double d, float e, float f) {
        int r = 0;
        if (a < b) r++;
        if (c > d) r += 2;
        if (e >= f) r += 4;
        if (c != c) r += 8;
        if (a == b) r += 16;
        return r;
    }

    long incrementField() { return lf++; }
    long preIncField() { return ++lf; }
    double compound() { df += 2.5; df *= 3; return df; }
    static long incStatic() { return sl++ + ++sl; }
    int arrInc(int i) { return ia[i]++ + ++ia[i + 1]; }
    long arrLongInc(int i) { return la[i]++; }
    long arrLongCompound(int i) { la[i] += 7L; la[i] <<= 2; return la[i]; }
    void arrStore(int i, long v) { la[i++] = v; ia[i] = (int) v; }
    static int swapLike(int a, int b) { int t = a; a = b; b = t; return a - b; }
    static boolean flags(boolean a, boolean b) { return (a && !b) || (b ^ a); }
    static int ternary(int x) { return x > 0 ? x : -x; }
    static int bitOps(int x) { return (x << 3) ^ (x >> 2) ^ (x >>> 1) ^ (~x) ^ (x & 0xff) ^ (x | 0x100); }
    static long constants() { return 0L + 1L + 5L + 0x7fffffffffffffffL + 1234567890123L; }
    static double dconst() { return 0.0 + 1.0 + 2.0 + 3.5 + Double.NaN + Float.MAX_VALUE; }
    static float fconst() { return 0f + 1f + 2f + 3.25f; }
    static int iconst() { return -1 + 0 + 1 + 5 + 6 + 127 + 128 + 32767 + 32768 + 65536 + -129; }
    static int loop(int n) {
        int s = 0;
        for (int i = 0; i < n; i += 300) s += i;
        s += 1000;
        s -= 200;
        return s;
    }
    long incLongField(Arith o) { return o.lf++ + o.lf--; }
    static void swapper(Object a, Object b) {
        Object x = a;
        synchronized (x) { b.hashCode(); }
    }
}
