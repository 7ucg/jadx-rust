package corpus;

public class Inner {
    private int field = 3;
    private static int sfield = 4;

    class Member {
        int get() { return field + sfield; }
        class Deeper { int twice() { return get() * 2; } }
    }

    static class Nested {
        private final String name;
        Nested(String name) { this.name = name; }
        String name() { return name; }
    }

    interface Cb { void call(int v); }

    Runnable anon() {
        final int local = 7;
        return new Runnable() {
            int count;
            @Override public void run() {
                count += local + field;
                System.out.println(count);
            }
        };
    }

    int local(int n) {
        class Local {
            int mul() { return n * field; }
        }
        return new Local().mul();
    }

    Cb callback() {
        return v -> field += v;
    }

    private int secret() { return field; }
    static int peek(Inner i) { return i.secret() + new Inner().new Member().get(); }
}
