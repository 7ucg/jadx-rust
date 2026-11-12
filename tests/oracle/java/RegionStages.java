import jadx.core.dex.nodes.MethodNode;
import jadx.core.dex.visitors.regions.CheckRegions;
import jadx.core.dex.visitors.regions.CleanRegions;
import jadx.core.dex.visitors.regions.IfRegionVisitor;
import jadx.core.dex.visitors.regions.LoopRegionVisitor;
import jadx.core.dex.visitors.regions.RegionMakerVisitor;
import jadx.core.dex.visitors.regions.ReturnVisitor;
import jadx.core.dex.visitors.regions.SwitchBreakVisitor;
import jadx.core.dex.visitors.shrink.CodeShrinkVisitor;

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
		if (stage >= 13) {
			new IfRegionVisitor().visit(mth);
		}
		if (stage >= 14) {
			new ReturnVisitor().visit(mth);
		}
		if (stage >= 15) {
			new CleanRegions().visit(mth);
		}
		if (stage >= 16) {
			new CodeShrinkVisitor().visit(mth);
		}
		if (stage >= 17) {
			new CheckRegions().visit(mth);
		}
		if (stage >= 18) {
			new LoopRegionVisitor().visit(mth);
		}
		if (stage >= 19) {
			new SwitchBreakVisitor().visit(mth);
		}
	}
}
