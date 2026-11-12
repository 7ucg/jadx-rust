package corpus;

public enum Enums {
    ONE(1, "one") {
        @Override int calc(int x) { return x + value; }
    },
    TWO(2, "two") {
        @Override int calc(int x) { return x * value; }
    },
    THREE(3, "three") {
        @Override int calc(int x) { return x - value; }
    };

    final int value;
    final String text;
    static final Enums DEFAULT = TWO;

    Enums(int v, String t) { value = v; text = t; }

    abstract int calc(int x);

    static Enums parse(String s) {
        for (Enums e : values()) {
            if (e.text.equals(s)) return e;
        }
        return DEFAULT;
    }

    public enum Simple { A, B, C }
}
