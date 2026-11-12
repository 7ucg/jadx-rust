package corpus;

import java.io.Serializable;
import java.util.*;

public class Generics<T extends Comparable<T> & Serializable, U> extends AbstractList<T> implements Comparable<Generics<T, U>> {
    private final List<T> items = new ArrayList<>();
    protected Map<String, ? extends List<? super T>> weird;
    public static final String NAME = "generics";

    @Override
    public T get(int index) { return items.get(index); }

    @Override
    public int size() { return items.size(); }

    @Override
    public boolean add(T t) { return items.add(t); }

    @Override
    public int compareTo(Generics<T, U> o) { return Integer.compare(size(), o.size()); }

    public <R extends Number> R convert(U in, Class<R> cls) throws ReflectiveOperationException {
        return cls.getConstructor(String.class).newInstance(String.valueOf(in));
    }

    public static <K, V extends Comparable<V>> K maxKey(Map<K, V> map) {
        K best = null;
        V bv = null;
        for (Map.Entry<K, V> e : map.entrySet()) {
            if (bv == null || e.getValue().compareTo(bv) > 0) {
                best = e.getKey();
                bv = e.getValue();
            }
        }
        return best;
    }

    public T[] toTyped(T[] arr) { return items.toArray(arr); }

    interface Visitor<R> { R visit(Generics<?, ?> g); }

    <R> R accept(Visitor<R> v) { return v.visit(this); }
}
