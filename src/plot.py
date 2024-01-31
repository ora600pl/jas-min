import plotly.express as px
import requests
import pandas as pd 
from dateutil.parser import parse

json_awr = requests.get("http://localhost:6751/parsedir/awrrpt").json()

load_profile_df = pd.json_normalize(json_awr, ['awr_doc', ['load_profile']])
load_profile_df['begin_snap_time'] = load_profile_df['begin_snap_time'].apply(lambda x: parse(x))
load_profile_df = load_profile_df.sort_values('begin_snap_time')

fig = px.line(load_profile_df, x='begin_snap_time', y='per_second', color='stat_name')
fig.show()