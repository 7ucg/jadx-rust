package corpus;

import java.io.*;

public class TryCatch {
    private final Object lock = new Object();
    private int counter;

    static int simple(String s) {
        try {
            return Integer.parseInt(s);
        } catch (NumberFormatException e) {
            return -1;
        }
    }

    static int finallyBlock(int x) {
        int r = 0;
        try {
            r = 10 / x;
        } catch (ArithmeticException e) {
            r = -1;
        } finally {
            r += 1000;
        }
        return r;
    }

    static void multiCatch(Object o) {
        try {
            String s = (String) o;
            System.out.println(s.length());
        } catch (ClassCastException | NullPointerException e) {
            System.out.println("bad " + e.getMessage());
        } catch (RuntimeException e) {
            throw e;
        }
    }

    static String tryWithResources(String name) throws IOException {
        try (BufferedReader r = new BufferedReader(new FileReader(name));
             StringWriter w = new StringWriter()) {
            String line;
            while ((line = r.readLine()) != null) {
                w.write(line);
            }
            return w.toString();
        }
    }

    static int nested(int a, int b) {
        try {
            try {
                return a / b;
            } finally {
                System.out.println("inner");
            }
        } catch (Exception e) {
            return -2;
        } finally {
            System.out.println("outer");
        }
    }

    int sync(int x) {
        synchronized (lock) {
            counter += x;
            if (counter > 10) {
                throw new IllegalStateException("too big");
            }
            return counter;
        }
    }

    synchronized void syncMethod() {
        counter++;
    }

    static int loopTry(int[] data) {
        int sum = 0;
        for (int i = 0; i < data.length; i++) {
            try {
                if (data[i] < 0) continue;
                sum += data[i];
                if (sum > 100) break;
            } finally {
                sum++;
            }
        }
        return sum;
    }

    static void thrower() throws IOException, InterruptedException {
        Thread.sleep(1);
        throw new FileNotFoundException("x");
    }
}
