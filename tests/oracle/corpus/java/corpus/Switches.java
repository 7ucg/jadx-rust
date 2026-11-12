package corpus;

public class Switches {
    enum Color { RED, GREEN, BLUE }

    sealed interface Shape permits Circle, Square {}
    record Circle(double r) implements Shape {}
    record Square(double side) implements Shape {}

    public static int dense(int x) {
        switch (x) {
            case 0: return 10;
            case 1: return 11;
            case 2: return 12;
            case 4: return 14;
            default: return -1;
        }
    }

    public static String sparse(int x) {
        switch (x) {
            case 1: return "one";
            case 100: return "hundred";
            case 10000: return "ten thousand";
            case -5: return "minus five";
            default: return "other";
        }
    }

    public static int fallThrough(int x) {
        int r = 0;
        switch (x) {
            case 1: r += 1;
            case 2: r += 2; break;
            case 3: r += 3;
            default: r += 100;
        }
        return r;
    }

    public static int stringSwitch(String s) {
        switch (s) {
            case "alpha": return 1;
            case "beta": return 2;
            case "Aa": return 3;
            case "BB": return 4;
            default: return 0;
        }
    }

    public static int enumSwitch(Color c) {
        switch (c) {
            case RED: return 1;
            case GREEN: return 2;
            default: return 3;
        }
    }

    public static String arrowSwitch(Color c) {
        return switch (c) {
            case RED -> "r";
            case GREEN -> "g";
            case BLUE -> "b";
        };
    }

    public static double patternSwitch(Shape s) {
        return switch (s) {
            case Circle c -> Math.PI * c.r() * c.r();
            case Square q -> q.side() * q.side();
        };
    }

    public static int charSwitch(char c) {
        switch (c) {
            case 'a': case 'e': case 'i': case 'o': case 'u': return 1;
            case 'y': return 2;
            default: return 0;
        }
    }

    public static int yieldSwitch(int k) {
        return switch (k) {
            case 1, 2, 3 -> {
                int t = k * 2;
                yield t + 1;
            }
            case 7 -> 70;
            default -> {
                if (k < 0) yield -1;
                yield 0;
            }
        };
    }
}
