package corpus;

import java.lang.annotation.*;

public class Annot {
    @Retention(RetentionPolicy.RUNTIME)
    @java.lang.annotation.Target({ElementType.TYPE, ElementType.METHOD, ElementType.FIELD, ElementType.PARAMETER})
    public @interface Info {
        String name() default "dflt";
        int level() default 3;
        long big() default 1L << 40;
        double ratio() default 0.5;
        float f() default 1.5f;
        boolean flag() default true;
        char c() default 'x';
        byte b() default 7;
        short s() default -3;
        Class<?> type() default Object.class;
        Class<?>[] types() default {String.class, int.class, int[].class};
        RetentionPolicy policy() default RetentionPolicy.CLASS;
        String[] tags() default {"a", "b"};
        Nested nested() default @Nested(5);
        Nested[] nesteds() default {};
    }

    @Retention(RetentionPolicy.RUNTIME)
    public @interface Nested { int value(); }

    @Retention(RetentionPolicy.CLASS)
    public @interface Invisible { String value() default ""; }

    @Retention(RetentionPolicy.SOURCE)
    public @interface Source {}

    @Deprecated
    @Info(name = "cls", level = 9, tags = {"x"}, nesteds = {@Nested(1), @Nested(2)})
    @Invisible("inv")
    @Source
    public static class Holder {
        @Info(flag = false, policy = RetentionPolicy.RUNTIME, type = Annot.class)
        @Invisible
        public int field;

        @Info
        @Deprecated
        public void method(@Nested(1) @Invisible("p") String a, int b, @Invisible int c) {}

        public Holder(@Info(name = "ctor") String s) {}
    }
}
