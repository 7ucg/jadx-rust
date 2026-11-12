.class public Lcorpus/Finally;
.super Ljava/lang/Object;
.source "Finally.smali"

.method public static cleanup()V
    .registers 0
    return-void
.end method

.method public static g(I)I
    .registers 1
    return p0
.end method

.method public static h()V
    .registers 0
    return-void
.end method

.method public static twoExits(I)I
    .registers 3

    :try_start_0
    if-lez p0, :cond_0

    invoke-static {p0}, Lcorpus/Finally;->g(I)I

    move-result v0
    :try_end_0
    .catchall {:try_start_0 .. :try_end_0} :catchall_0

    invoke-static {}, Lcorpus/Finally;->cleanup()V

    return v0

    :cond_0
    :try_start_1
    invoke-static {}, Lcorpus/Finally;->h()V
    :try_end_1
    .catchall {:try_start_1 .. :try_end_1} :catchall_0

    invoke-static {}, Lcorpus/Finally;->cleanup()V

    const/4 v0, 0x1

    return v0

    :catchall_0
    move-exception v0

    invoke-static {}, Lcorpus/Finally;->cleanup()V

    throw v0
.end method

.method public static withCatch(I)V
    .registers 3

    :try_start_0
    invoke-static {p0}, Lcorpus/Finally;->g(I)I
    :try_end_0
    .catch Ljava/lang/RuntimeException; {:try_start_0 .. :try_end_0} :catch_0
    .catchall {:try_start_0 .. :try_end_0} :catchall_0

    invoke-static {}, Lcorpus/Finally;->cleanup()V

    return-void

    :catch_0
    move-exception v0

    :try_start_1
    invoke-static {}, Lcorpus/Finally;->h()V
    :try_end_1
    .catchall {:try_start_1 .. :try_end_1} :catchall_0

    invoke-static {}, Lcorpus/Finally;->cleanup()V

    return-void

    :catchall_0
    move-exception v0

    invoke-static {}, Lcorpus/Finally;->cleanup()V

    throw v0
.end method
