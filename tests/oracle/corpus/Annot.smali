.class public final enum Lcorpus/Annot;
.super Ljava/lang/Enum;
.source "Annot.smali"

.implements Ljava/io/Serializable;
.implements Ljava/lang/Comparable;

.annotation runtime Ljava/lang/annotation/Retention;
    value = .enum Ljava/lang/annotation/RetentionPolicy;->RUNTIME:Ljava/lang/annotation/RetentionPolicy;
.end annotation

.annotation system Ldalvik/annotation/Signature;
    value = {
        "Ljava/lang/Enum<",
        "Lcorpus/Annot;",
        ">;"
    }
.end annotation

.annotation build Lcorpus/Every;
    b = 0x1t
    s = -0x2s
    c = 'x'
    d = 'Ā'
    i = -0x3
    l = 0x7fffffffffffffffL
    f = 1.5f
    dd = 2.25
    flag = true
    nothing = null
    str = "a\"b'c"
    type = Lcorpus/Annot;
    fld = Lcorpus/Annot;->A:Lcorpus/Annot;
    mth = Lcorpus/Annot;->m(I)V
    empty = {}
    arr = {
        0x1,
        0x2
    }
    nested = .subannotation Lcorpus/Inner;
        k = "v"
    .end subannotation
.end annotation

.field public static final enum A:Lcorpus/Annot;
.field public static final B:I = 0x5
.field public static final C:Ljava/lang/String; = "const"
.field public static final D:D = 3.14
.field public static final E:F = -0.0f
.field public static final F:J = -0x1L
.field public static final G:Z = false
.field public static final H:C = 'q'
.field public static final I:B = 0x7ft
.field public static final J:S = -0x8000s
.field public static K:I = 0x0
.field public static final L:I = 0x0
.field public static final M:I = 0x9
.field public static final N:Ljava/lang/Object;
.field private foo:I
    .annotation runtime Ljava/lang/Deprecated;
    .end annotation
.end field

.method static constructor <clinit>()V
    .registers 2
    const/4 v0, 0x1
    sput v0, Lcorpus/Annot;->L:I
    sput v0, Lcorpus/Annot;->M:I
    const/4 v0, 0x0
    sput-object v0, Lcorpus/Annot;->N:Ljava/lang/Object;
    return-void
.end method

.method public annotated(Ljava/lang/String;IJ[I)V
    .registers 8
    .annotation runtime Ljava/lang/Deprecated;
    .end annotation
    .annotation system Ldalvik/annotation/Throws;
        value = {
            Ljava/io/IOException;
        }
    .end annotation
    .param p1    # Ljava/lang/String;
        .annotation runtime Lcorpus/Nullable;
        .end annotation
    .end param
    .param p3    # J
        .annotation runtime Lcorpus/Nullable;
        .end annotation
        .annotation runtime Lcorpus/Other;
            v = 0x1
        .end annotation
    .end param
    return-void
.end method

.method public static native nat()V
.end method
