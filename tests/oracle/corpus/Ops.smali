.class public Lcorpus/Ops;
.super Ljava/lang/Object;
.source "Ops.smali"

.field public static s:S
.field public static sb:B
.field public static c:C
.field public static z:Z
.field public static j:J
.field public static d:D
.field public static f:F
.field public static o:Ljava/lang/Object;
.field public i:I

.method public static moves(IJLjava/lang/Object;)V
    .registers 20
    move/16 v1, v2
    move-wide/16 v2, v4
    move-object/16 v6, p2
    move v1, v2
    move-wide v4, v6
    move-object v1, v2
    move/from16 v1, v17
    move-wide/from16 v1, v16
    move-object/from16 v1, v17
    return-void
.end method

.method public static fields()V
    .registers 4
    sget-short v0, Lcorpus/Ops;->s:S
    sput-short v0, Lcorpus/Ops;->s:S
    sget-byte v0, Lcorpus/Ops;->sb:B
    sput-byte v0, Lcorpus/Ops;->sb:B
    sget-char v0, Lcorpus/Ops;->c:C
    sput-char v0, Lcorpus/Ops;->c:C
    sget-boolean v0, Lcorpus/Ops;->z:Z
    sput-boolean v0, Lcorpus/Ops;->z:Z
    sget-wide v0, Lcorpus/Ops;->j:J
    sput-wide v0, Lcorpus/Ops;->j:J
    sget-object v0, Lcorpus/Ops;->o:Ljava/lang/Object;
    sput-object v0, Lcorpus/Ops;->o:Ljava/lang/Object;
    return-void
.end method

.method public static jumps(I)I
    .registers 3
    goto/32 :far
    :back
    return p0
    :far
    goto/16 :mid
    :mid
    if-eqz p0, :back
    if-ne p0, p0, :back
    goto :back
.end method

.method public static consts()V
    .registers 8
    const/4 v0, -0x8
    const/16 v0, -0x8000
    const v0, 0x7fffffff
    const v0, -0x80000000
    const/high16 v0, 0x3f800000
    const/high16 v0, -0x40800000
    const/high16 v0, 0x7f800000
    const/high16 v0, 0x7fc00000
    const v0, 0x40490fdb
    const v0, 0x402df854
    const v0, 0x7f7fffff
    const-wide/16 v0, -0x1
    const-wide/32 v0, 0x7fffffff
    const-wide v0, 0x7fffffffL
    const-wide v0, -0x8000000000000000L
    const-wide v0, 0x400921fb54442d18L
    const-wide v0, 0x4005bf0a8b145769L
    const-wide v0, 0x7ff0000000000000L
    const-wide/high16 v0, 0x3ff0000000000000L
    const-wide/high16 v0, 0x7ff8000000000000L
    const-string v0, "tab\there\nnl\rcr \"q\" 'a' \\ \u0001 \u007f é €"
    const-string/jumbo v0, ""
    const-class v0, [[I
    return-void
.end method

.method public static arith(II)V
    .registers 6
    add-int/lit8 v0, p0, -0x80
    rsub-int/lit8 v0, p0, 0x7f
    add-int/lit16 v0, p0, -0x8000
    rsub-int v0, p0, 0x7fff
    shl-int/lit8 v0, p0, 0x1f
    ushr-int/lit8 v0, p0, 0x1
    add-int/2addr v0, p1
    neg-int v0, p0
    int-to-long v0, p0
    long-to-double v0, v2
    cmpl-float v0, v1, v2
    cmp-long v0, v2, v3
    return-void
.end method

.method public static switches(I)I
    .registers 4
    packed-switch p0, :pdata
    sparse-switch p0, :sdata
    const/4 v0, 0x0
    return v0
    :p1
    const/4 v0, 0x1
    return v0
    :s1
    const/4 v0, 0x2
    return v0
    :pdata
    .packed-switch -0x1
        :p1
        :s1
    .end packed-switch
    :sdata
    .sparse-switch
        -0x80000000 -> :p1
        0x7fffffff -> :s1
    .end sparse-switch
.end method

.method public static arrays()V
    .registers 4
    const/4 v0, 0x4
    new-array v1, v0, [B
    fill-array-data v1, :a1
    new-array v1, v0, [S
    fill-array-data v1, :a2
    new-array v1, v0, [J
    fill-array-data v1, :a3
    new-array v1, v0, [F
    fill-array-data v1, :a4
    new-array v1, v0, [Z
    fill-array-data v1, :a5
    filled-new-array {v0, v0}, [I
    filled-new-array/range {v0 .. v1}, [I
    filled-new-array {}, [I
    return-void
    :a1
    .array-data 1
        -0x1t
        0x7ft
        0x0t
    .end array-data
    :a2
    .array-data 2
        -0x8000s
        0x7fffs
    .end array-data
    :a3
    .array-data 8
        0x7fffffffffffffffL
        -0x1
        0x3ff0000000000000L
    .end array-data
    :a4
    .array-data 4
        0x3f800000
        0x0
        -0x1
    .end array-data
    :a5
    .array-data 1
        0x1t
    .end array-data
.end method

.method public static tries()V
    .registers 3
    :ts1
    invoke-static {}, Lcorpus/Ops;->tries()V
    :te1
    .catch Ljava/io/IOException; {:ts1 .. :te1} :h1
    .catch Ljava/lang/Exception; {:ts1 .. :te1} :h2
    .catchall {:ts1 .. :te1} :h3
    return-void
    :h1
    move-exception v0
    return-void
    :h2
    move-exception v0
    :ts2
    invoke-static {}, Lcorpus/Ops;->tries()V
    :te2
    .catchall {:ts2 .. :te2} :h3
    return-void
    :h3
    move-exception v0
    throw v0
.end method

.method public static newer()V
    .registers 4
    const-method-handle v0, invoke-static@Lcorpus/Ops;->tries()V
    const-method-handle v0, static-get@Lcorpus/Ops;->s:S
    const-method-type v0, (IJ)V
    invoke-polymorphic {v0, v1}, Ljava/lang/invoke/MethodHandle;->invoke([Ljava/lang/Object;)Ljava/lang/Object;, (I)V
    invoke-polymorphic/range {v0 .. v2}, Ljava/lang/invoke/MethodHandle;->invokeExact([Ljava/lang/Object;)Ljava/lang/Object;, (II)V
    invoke-custom {v0}, call_site_0("run", (I)V, "extra", 0x1, 1.5f)@Lcorpus/Ops;->boot(Ljava/lang/invoke/MethodHandles$Lookup;Ljava/lang/String;Ljava/lang/invoke/MethodType;Ljava/lang/String;IF)Ljava/lang/invoke/CallSite;
    invoke-custom/range {v0 .. v1}, call_site_1("run2", (II)V)@Lcorpus/Ops;->boot2(Ljava/lang/invoke/MethodHandles$Lookup;Ljava/lang/String;Ljava/lang/invoke/MethodType;)Ljava/lang/invoke/CallSite;
    return-void
.end method

.method public abstract native synchronized strictfp bridge varargs foo(JD[I)V
.end method
