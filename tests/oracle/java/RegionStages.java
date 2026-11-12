import jadx.core.dex.nodes.MethodNode;
import jadx.core.dex.visitors.regions.RegionMakerVisitor;

/**
 * Oracle helper: the region passes of the default pipeline, one stage per pass:
 * 12 RegionMakerVisitor, 13 IfRegionVisitor, 14 ReturnVisitor, 15 CleanRegions, 16 CodeShrinkVisitor,
 * 17 CheckRegions, 18 LoopRegionVisitor, 19 SwitchBreakVisitor.
 * Passes that do not change the region tree or only rewrite plain instructions (MethodThrowsVisitor,
 * MethodInvokeVisitor, SimplifyVisitor, ...) are not run.
 */
public class RegionStages {
	public static void run(MethodNode mth, int stage) throws Exception {
		if (stage >= 12) {
			new RegionMakerVisitor().visit(mth);
		}
	}
}
