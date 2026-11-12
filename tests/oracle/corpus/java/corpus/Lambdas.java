package corpus;

import java.util.*;
import java.util.function.*;
import java.util.stream.*;

public class Lambdas {
    interface Op { int apply(int a, int b); }

    record Point(int x, int y, String label) {}

    private int base = 5;

    int useLambdas(List<String> in) {
        Function<String, Integer> len = s -> s.length() + base;
        Supplier<List<String>> sup = ArrayList::new;
        BinaryOperator<Integer> add = Integer::sum;
        Op mul = (a, b) -> a * b;
        Runnable r = () -> System.out.println("run " + base);
        r.run();
        List<String> l = sup.get();
        l.addAll(in);
        return l.stream().map(len).reduce(0, add) + mul.apply(2, 3);
    }

    static String concat(String a, int b, long c, double d, char e, Object o) {
        return a + b + "-" + c + ":" + d + e + o + "\u0001tail";
    }

    static List<Integer> streams(int n) {
        return IntStream.range(0, n).boxed().filter(i -> i % 2 == 0).collect(Collectors.toList());
    }

    static Comparator<String> cmp() {
        return Comparator.comparing(String::length).thenComparing(Comparator.naturalOrder());
    }

    Object capture(int a, String b) {
        return (Supplier<String>) () -> b + a + this.base;
    }

    String record() {
        Point p = new Point(1, 2, "pt");
        return p + " " + p.hashCode() + " " + p.equals(new Point(1, 2, "pt"));
    }
}
